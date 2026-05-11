//! JWT/JWKS validation for SSO-enabled deployments.
//!
//! Provides:
//! - `JwksCache`: in-memory JWKS key cache with configurable TTL and auto-refresh
//! - `JwtValidator`: validates JWT tokens against cached JWKS keys
//! - `JwtRequestContextExtractor`: axum extractor that reads `Authorization: Bearer <token>`
//!   and injects user identity into request extensions
//!
//! Implements requirements R26.12, R26.13.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::SsoConfig;

// ── JWKS types ─────────────────────────────────────────────────────

/// A single JSON Web Key from a JWKS endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// Key type (e.g. "RSA").
    pub kty: String,
    /// Key ID — used to match against the JWT header `kid`.
    pub kid: Option<String>,
    /// Algorithm (e.g. "RS256").
    #[allow(dead_code)] // Deserialized from JWKS endpoint; used for key selection
    pub alg: Option<String>,
    /// RSA modulus (base64url-encoded).
    pub n: Option<String>,
    /// RSA exponent (base64url-encoded).
    pub e: Option<String>,
    /// Key use (e.g. "sig").
    #[serde(rename = "use")]
    #[allow(dead_code)] // Deserialized from JWKS endpoint; used for key filtering
    pub key_use: Option<String>,
}

/// JWKS document returned by the identity provider.
#[derive(Debug, Clone, Deserialize)]
pub struct JwksDocument {
    pub keys: Vec<Jwk>,
}

// ── JwksCache ──────────────────────────────────────────────────────

/// Fetches and caches JWKS keys with a configurable TTL.
///
/// On unknown `kid`, fetches fresh JWKS before rejecting the token,
/// supporting seamless key rotation.
pub struct JwksCache {
    jwks_url: String,
    ttl: Duration,
    client: reqwest::Client,
    inner: RwLock<CacheInner>,
}

struct CacheInner {
    /// kid → DecodingKey
    keys: HashMap<String, DecodingKey>,
    /// When the cache was last refreshed.
    last_refresh: Option<Instant>,
}

impl JwksCache {
    /// Create a new JWKS cache.
    ///
    /// `ttl` controls how long cached keys are considered fresh (default: 1 hour).
    pub fn new(jwks_url: String, ttl: Duration, client: reqwest::Client) -> Self {
        Self {
            jwks_url,
            ttl,
            client,
            inner: RwLock::new(CacheInner {
                keys: HashMap::new(),
                last_refresh: None,
            }),
        }
    }

    /// Create with the default 1-hour TTL.
    pub fn with_default_ttl(jwks_url: String, client: reqwest::Client) -> Self {
        Self::new(jwks_url, Duration::from_secs(3600), client)
    }

    /// Look up a `DecodingKey` by `kid`.
    ///
    /// 1. If the cache is stale (past TTL), refresh first.
    /// 2. If the `kid` is not found, force-refresh once before giving up.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, JwtError> {
        // Check if cache needs a TTL-based refresh.
        {
            let inner = self.inner.read().await;
            let needs_refresh = match inner.last_refresh {
                Some(t) => t.elapsed() >= self.ttl,
                None => true,
            };
            if !needs_refresh {
                if let Some(key) = inner.keys.get(kid) {
                    return Ok(key.clone());
                }
            }
        }

        // Refresh and try again.
        self.refresh().await?;

        let inner = self.inner.read().await;
        if let Some(key) = inner.keys.get(kid) {
            return Ok(key.clone());
        }

        // kid still not found after fresh fetch — unknown key.
        Err(JwtError::UnknownKid(kid.to_string()))
    }

    /// Fetch JWKS from the configured URL and update the cache.
    pub async fn refresh(&self) -> Result<(), JwtError> {
        let resp = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| JwtError::JwksFetchFailed(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(JwtError::JwksFetchFailed(format!("HTTP {}", resp.status())));
        }

        let doc: JwksDocument = resp
            .json()
            .await
            .map_err(|e| JwtError::JwksFetchFailed(e.to_string()))?;

        let mut keys = HashMap::new();
        for jwk in &doc.keys {
            if let Some(ref kid) = jwk.kid {
                if let Some(dk) = decoding_key_from_jwk(jwk) {
                    keys.insert(kid.clone(), dk);
                }
            }
        }

        let mut inner = self.inner.write().await;
        inner.keys = keys;
        inner.last_refresh = Some(Instant::now());
        Ok(())
    }

    /// Directly set cached keys (useful for testing without network).
    #[allow(dead_code)] // Used in tests for seeding JWKS cache without network
    pub async fn set_keys(&self, keys: HashMap<String, DecodingKey>) {
        let mut inner = self.inner.write().await;
        inner.keys = keys;
        inner.last_refresh = Some(Instant::now());
    }
}

/// Build a `DecodingKey` from a JWK (RSA only for now).
fn decoding_key_from_jwk(jwk: &Jwk) -> Option<DecodingKey> {
    if jwk.kty != "RSA" {
        return None;
    }
    let n = jwk.n.as_deref()?;
    let e = jwk.e.as_deref()?;
    DecodingKey::from_rsa_components(n, e).ok()
}

// ── JWT Claims ─────────────────────────────────────────────────────

/// Claims extracted from a validated JWT token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject — typically the user ID.
    pub sub: String,
    /// Email address (optional).
    pub email: Option<String>,
    /// Roles from the token (optional).
    #[serde(default)]
    pub roles: Vec<String>,
    /// Scopes from the token (optional, space-delimited in the raw claim).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Issuer.
    pub iss: Option<String>,
    /// Audience.
    pub aud: Option<String>,
}

/// Internal raw claims for deserialization — handles the various
/// ways providers encode roles and scopes.
#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    email: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
    /// Some providers use a space-delimited `scope` string.
    scope: Option<String>,
    iss: Option<String>,
    aud: Option<serde_json::Value>,
}

impl RawClaims {
    fn into_jwt_claims(self) -> JwtClaims {
        let scopes = match self.scope {
            Some(s) => s.split_whitespace().map(String::from).collect(),
            None => vec![],
        };
        JwtClaims {
            sub: self.sub,
            email: self.email,
            roles: self.roles,
            scopes,
            iss: self.iss,
            aud: self.aud.map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            }),
        }
    }
}

// ── JwtValidator ───────────────────────────────────────────────────

/// Validates JWT tokens using JWKS-cached keys.
pub struct JwtValidator {
    cache: Arc<JwksCache>,
    issuer: String,
    audience: String,
}

impl JwtValidator {
    pub fn new(cache: Arc<JwksCache>, issuer: String, audience: String) -> Self {
        Self {
            cache,
            issuer,
            audience,
        }
    }

    /// Build from an `SsoConfig`.
    pub fn from_sso_config(sso: &SsoConfig, client: reqwest::Client) -> Self {
        let cache = Arc::new(JwksCache::with_default_ttl(sso.jwks_url.clone(), client));
        Self::new(cache, sso.issuer.clone(), sso.audience.clone())
    }

    /// Validate a raw JWT token string and return extracted claims.
    pub async fn validate(&self, token: &str) -> Result<JwtClaims, JwtError> {
        // Decode header to get `kid`.
        let header = decode_header(token).map_err(|e| JwtError::InvalidToken(e.to_string()))?;
        let kid = header
            .kid
            .ok_or_else(|| JwtError::InvalidToken("missing kid in JWT header".into()))?;

        // Fetch the decoding key.
        let decoding_key = self.cache.get_key(&kid).await?;

        // Build validation parameters.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);

        let token_data = decode::<RawClaims>(token, &decoding_key, &validation)
            .map_err(|e| JwtError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims.into_jwt_claims())
    }
}

// ── JwtRequestContextExtractor ─────────────────────────────────────

/// Axum extractor that reads `Authorization: Bearer <token>`, validates
/// the JWT, and provides `JwtClaims` to handlers.
///
/// Requires `Arc<JwtValidator>` in the request extensions (typically
/// inserted via a middleware layer or as axum state).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used as an axum extractor in JWT-protected route handlers
pub struct JwtRequestContextExtractor(pub JwtClaims);

impl<S> FromRequestParts<S> for JwtRequestContextExtractor
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Try to get pre-validated claims from extensions first
        // (set by middleware).
        if let Some(claims) = parts.extensions.get::<JwtClaims>() {
            return Ok(Self(claims.clone()));
        }

        Err((
            StatusCode::UNAUTHORIZED,
            "missing or invalid JWT token".to_string(),
        ))
    }
}

/// Axum middleware layer that validates JWT tokens and injects claims
/// into request extensions.
///
/// Usage:
/// ```ignore
/// let jwt_layer = axum::middleware::from_fn_with_state(
///     validator.clone(),
///     jwt_auth_middleware,
/// );
/// ```
pub async fn jwt_auth_middleware(
    axum::extract::State(validator): axum::extract::State<Arc<JwtValidator>>,
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => {
            return axum::response::IntoResponse::into_response((
                StatusCode::UNAUTHORIZED,
                "missing Authorization: Bearer <token> header",
            ));
        }
    };

    match validator.validate(token).await {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "JWT validation failed");
            axum::response::IntoResponse::into_response((
                StatusCode::UNAUTHORIZED,
                format!("invalid token: {e}"),
            ))
        }
    }
}

// ── Errors ─────────────────────────────────────────────────────────

/// Errors that can occur during JWT/JWKS operations.
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("JWKS fetch failed: {0}")]
    JwksFetchFailed(String),

    #[error("unknown kid: {0}")]
    UnknownKid(String),

    #[error("invalid token: {0}")]
    InvalidToken(String),
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use serde_json::json;

    /// Generate an RSA key pair for testing.
    fn test_rsa_keys() -> (EncodingKey, DecodingKey, String, String) {
        // Use a pre-generated 2048-bit RSA key pair for deterministic tests.
        // In real usage these come from the IdP's JWKS endpoint.
        let rsa_private = include_str!("../tests/fixtures/test_rsa_private.pem");
        let rsa_public = include_str!("../tests/fixtures/test_rsa_public.pem");
        let encoding_key = EncodingKey::from_rsa_pem(rsa_private.as_bytes()).unwrap();
        let decoding_key = DecodingKey::from_rsa_pem(rsa_public.as_bytes()).unwrap();

        // Extract n and e components for JWKS simulation.
        // We'll use the DecodingKey directly in tests.
        (
            encoding_key,
            decoding_key,
            rsa_private.to_string(),
            rsa_public.to_string(),
        )
    }

    fn make_token(encoding_key: &EncodingKey, kid: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, encoding_key).unwrap()
    }

    #[tokio::test]
    async fn validate_valid_token() {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();
        let kid = "test-key-1";

        let claims = json!({
            "sub": "user-123",
            "email": "user@example.com",
            "roles": ["admin"],
            "scope": "read:data write:data",
            "iss": "https://auth.example.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "iat": chrono::Utc::now().timestamp(),
        });

        let token = make_token(&encoding_key, kid, &claims);

        // Set up cache with the test key.
        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        let result = validator.validate(&token).await.unwrap();
        assert_eq!(result.sub, "user-123");
        assert_eq!(result.email.as_deref(), Some("user@example.com"));
        assert_eq!(result.roles, vec!["admin"]);
        assert_eq!(result.scopes, vec!["read:data", "write:data"]);
    }

    #[tokio::test]
    async fn reject_expired_token() {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();
        let kid = "test-key-1";

        let claims = json!({
            "sub": "user-123",
            "iss": "https://auth.example.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() - 3600, // expired
            "iat": chrono::Utc::now().timestamp() - 7200,
        });

        let token = make_token(&encoding_key, kid, &claims);

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        let err = validator.validate(&token).await.unwrap_err();
        assert!(matches!(err, JwtError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn reject_wrong_issuer() {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();
        let kid = "test-key-1";

        let claims = json!({
            "sub": "user-123",
            "iss": "https://wrong-issuer.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "iat": chrono::Utc::now().timestamp(),
        });

        let token = make_token(&encoding_key, kid, &claims);

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        let err = validator.validate(&token).await.unwrap_err();
        assert!(matches!(err, JwtError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn reject_wrong_audience() {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();
        let kid = "test-key-1";

        let claims = json!({
            "sub": "user-123",
            "iss": "https://auth.example.com",
            "aud": "wrong-audience",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "iat": chrono::Utc::now().timestamp(),
        });

        let token = make_token(&encoding_key, kid, &claims);

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        let err = validator.validate(&token).await.unwrap_err();
        assert!(matches!(err, JwtError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn reject_unknown_kid() {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();

        let claims = json!({
            "sub": "user-123",
            "iss": "https://auth.example.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "iat": chrono::Utc::now().timestamp(),
        });

        // Sign with kid "unknown-key" but cache only has "test-key-1".
        let token = make_token(&encoding_key, "unknown-key", &claims);

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert("test-key-1".to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        // This will try to refresh (which will fail since URL is fake),
        // then return UnknownKid or JwksFetchFailed.
        let err = validator.validate(&token).await.unwrap_err();
        // The refresh will fail because the URL is fake, so we get a fetch error.
        assert!(
            matches!(err, JwtError::JwksFetchFailed(_) | JwtError::UnknownKid(_)),
            "expected JwksFetchFailed or UnknownKid, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn token_without_kid_rejected() {
        let (encoding_key, _, _, _) = test_rsa_keys();

        let claims = json!({
            "sub": "user-123",
            "iss": "https://auth.example.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() + 3600,
        });

        // Token without kid.
        let header = Header::new(Algorithm::RS256);
        let token = encode(&header, &claims, &encoding_key).unwrap();

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));

        let validator = JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        );

        let err = validator.validate(&token).await.unwrap_err();
        assert!(matches!(err, JwtError::InvalidToken(_)));
    }

    #[test]
    fn raw_claims_parses_scope_string() {
        let raw: RawClaims = serde_json::from_value(json!({
            "sub": "u1",
            "scope": "read:x write:y admin:z"
        }))
        .unwrap();
        let claims = raw.into_jwt_claims();
        assert_eq!(claims.scopes, vec!["read:x", "write:y", "admin:z"]);
    }

    #[test]
    fn raw_claims_handles_missing_optional_fields() {
        let raw: RawClaims = serde_json::from_value(json!({
            "sub": "u1"
        }))
        .unwrap();
        let claims = raw.into_jwt_claims();
        assert_eq!(claims.sub, "u1");
        assert!(claims.email.is_none());
        assert!(claims.roles.is_empty());
        assert!(claims.scopes.is_empty());
    }

    #[test]
    fn decoding_key_from_jwk_rejects_non_rsa() {
        let jwk = Jwk {
            kty: "EC".into(),
            kid: Some("ec-key".into()),
            alg: Some("ES256".into()),
            n: None,
            e: None,
            key_use: Some("sig".into()),
        };
        assert!(decoding_key_from_jwk(&jwk).is_none());
    }

    // ── JWT middleware integration tests (Task 11.1) ───────────────

    /// Helper: build a JwtValidator with a pre-seeded key cache for middleware tests.
    async fn test_validator_and_token() -> (Arc<JwtValidator>, String) {
        let (encoding_key, decoding_key, _, _) = test_rsa_keys();
        let kid = "mw-test-key";

        let claims = serde_json::json!({
            "sub": "mw-user",
            "email": "mw@example.com",
            "roles": ["viewer"],
            "scope": "read:all",
            "iss": "https://auth.example.com",
            "aud": "my-gateway",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "iat": chrono::Utc::now().timestamp(),
        });

        let token = make_token(&encoding_key, kid, &claims);

        let cache = Arc::new(JwksCache::new(
            "http://unused".into(),
            Duration::from_secs(3600),
            reqwest::Client::new(),
        ));
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), decoding_key);
        cache.set_keys(keys).await;

        let validator = Arc::new(JwtValidator::new(
            cache,
            "https://auth.example.com".into(),
            "my-gateway".into(),
        ));

        (validator, token)
    }

    /// Build a minimal axum app with jwt_auth_middleware protecting a test route.
    fn test_app(validator: Arc<JwtValidator>) -> axum::Router {
        let protected = axum::Router::new()
            .route("/protected", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                validator,
                jwt_auth_middleware,
            ));
        protected
    }

    #[tokio::test]
    async fn middleware_missing_auth_header_returns_401() {
        let (validator, _token) = test_validator_and_token().await;
        let app = test_app(validator);

        let req = axum::http::Request::builder()
            .uri("/protected")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn middleware_malformed_token_returns_401() {
        let (validator, _token) = test_validator_and_token().await;
        let app = test_app(validator);

        let req = axum::http::Request::builder()
            .uri("/protected")
            .header("Authorization", "Bearer not-a-real-jwt-token")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn middleware_valid_token_injects_claims() {
        let (validator, token) = test_validator_and_token().await;

        use axum::response::IntoResponse;

        // Build an app where the handler reads JwtClaims from extensions
        let app = axum::Router::new()
            .route(
                "/protected",
                axum::routing::get(|req: axum::http::Request<axum::body::Body>| async move {
                    let claims = req.extensions().get::<JwtClaims>().cloned();
                    match claims {
                        Some(c) => axum::Json(serde_json::json!({
                            "sub": c.sub,
                            "roles": c.roles,
                        }))
                        .into_response(),
                        None => (StatusCode::INTERNAL_SERVER_ERROR, "no claims").into_response(),
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                validator,
                jwt_auth_middleware,
            ));

        let req = axum::http::Request::builder()
            .uri("/protected")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["sub"], "mw-user");
        assert_eq!(json["roles"], serde_json::json!(["viewer"]));
    }
}

//! Property-based tests for browser domain restriction.
//!
//! Feature: full-stack-completion, Property 9: Browser domain restriction
//! **Validates: Requirements 4.5**
//!
//! For any URL and any `allowed_domains` list where the URL's domain is not
//! in the list, the browser tool SHALL reject the navigation with an error.

use adk_gateway::browser_factory::is_domain_allowed;
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate a valid domain name (lowercase, no special chars).
fn arb_domain() -> impl Strategy<Value = String> {
    "[a-z]{2,10}\\.[a-z]{2,5}"
}

/// Generate a list of allowed domains (1 to 5 entries).
fn arb_allowed_domains() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(arb_domain(), 1..6)
}

// ── Property 9: Browser domain restriction ─────────────────────────
// Feature: full-stack-completion, Property 9: Browser domain restriction
// **Validates: Requirements 4.5**
//
// For any URL and any `allowed_domains` list where the URL's domain is NOT
// in the list, the domain check SHALL reject the navigation.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// When a URL's domain is NOT in the allowed_domains list (and the list
    /// is non-empty), is_domain_allowed SHALL return false.
    #[test]
    fn domain_not_in_list_is_rejected(
        allowed_domains in arb_allowed_domains(),
        not_allowed_prefix in "[a-z]{2,8}",
    ) {
        // Construct a domain guaranteed not to be in the allowed list
        let not_allowed_domain = format!("{not_allowed_prefix}-blocked.zzz");

        // Ensure it's truly not in the list
        let is_actually_in_list = allowed_domains
            .iter()
            .any(|d| d.to_lowercase() == not_allowed_domain.to_lowercase());

        if !is_actually_in_list {
            let url = format!("https://{not_allowed_domain}/some/path");
            let result = is_domain_allowed(&url, &allowed_domains);
            prop_assert!(
                !result,
                "URL '{}' with domain '{}' should be REJECTED when allowed_domains={:?}",
                url, not_allowed_domain, allowed_domains
            );
        }
    }

    /// When a URL's domain IS in the allowed_domains list, is_domain_allowed
    /// SHALL return true.
    #[test]
    fn domain_in_list_is_allowed(
        allowed_domains in arb_allowed_domains(),
        path_suffix in "[a-z/]{0,20}",
    ) {
        // Pick the first domain from the allowed list
        let target_domain = &allowed_domains[0];
        let url = format!("https://{target_domain}/{path_suffix}");

        let result = is_domain_allowed(&url, &allowed_domains);
        prop_assert!(
            result,
            "URL '{}' with domain '{}' should be ALLOWED when allowed_domains={:?}",
            url, target_domain, allowed_domains
        );
    }

    /// When allowed_domains is empty, ALL URLs are allowed regardless of domain.
    #[test]
    fn empty_allowed_domains_allows_all(
        domain in arb_domain(),
        path_suffix in "[a-z/]{0,20}",
    ) {
        let url = format!("https://{domain}/{path_suffix}");
        let empty: Vec<String> = vec![];

        let result = is_domain_allowed(&url, &empty);
        prop_assert!(
            result,
            "URL '{}' should be ALLOWED when allowed_domains is empty",
            url
        );
    }

    /// Domain matching is case-insensitive: a URL with mixed-case domain
    /// should match an allowed domain regardless of case.
    #[test]
    fn domain_matching_is_case_insensitive(
        base_domain in arb_domain(),
        path_suffix in "[a-z/]{0,10}",
    ) {
        // Create an uppercase version of the domain for the allowed list
        let upper_domain = base_domain.to_uppercase();
        let allowed = vec![upper_domain];

        // URL uses lowercase domain
        let url = format!("https://{base_domain}/{path_suffix}");

        let result = is_domain_allowed(&url, &allowed);
        prop_assert!(
            result,
            "URL '{}' should match allowed domain '{}' case-insensitively",
            url, allowed[0]
        );
    }

    /// Invalid/unparseable URLs are rejected when allowed_domains is non-empty.
    #[test]
    fn invalid_urls_rejected_when_domains_restricted(
        allowed_domains in arb_allowed_domains(),
        garbage in "[^:/ ]{1,20}",
    ) {
        // Construct something that's not a valid URL (no scheme)
        let invalid_url = format!("not-a-valid-url-{garbage}");

        // Only test if it actually fails to parse as a URL
        if url::Url::parse(&invalid_url).is_err() {
            let result = is_domain_allowed(&invalid_url, &allowed_domains);
            prop_assert!(
                !result,
                "Invalid URL '{}' should be REJECTED when allowed_domains is non-empty",
                invalid_url
            );
        }
    }
}

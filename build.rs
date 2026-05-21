//! Build script — builds the React UI before compiling the gateway binary.
//!
//! Always rebuilds the UI when any source file in ui/src/ changes.
//! This ensures rust-embed always has fresh assets to embed.

use std::process::Command;

fn main() {
    // Always re-run when UI source changes
    println!("cargo:rerun-if-changed=ui/src/");
    println!("cargo:rerun-if-changed=ui/index.html");
    println!("cargo:rerun-if-changed=ui/package.json");
    println!("cargo:rerun-if-changed=ui/vite.config.ts");
    println!("cargo:rerun-if-changed=ui/tsconfig.json");

    // Extract adk-rust version from the local path dependency
    extract_adk_version();

    // Emit build timestamp so the binary can report when it was compiled
    let now = chrono_lite_now();
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", now);

    build_ui();
}

/// Get current UTC timestamp in ISO 8601 format without external deps.
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple UTC format: YYYY-MM-DDTHH:MM:SSZ
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    // Approximate date from days since epoch (good enough for build stamps)
    let (year, month, day) = days_to_date(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hours, minutes, seconds)
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Simplified date calculation from days since 1970-01-01
    let mut y = 1970;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1;
    for &days_in_month in &months {
        if remaining < days_in_month { break; }
        remaining -= days_in_month;
        m += 1;
    }
    (y, m, remaining + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Read the adk-rust workspace version and expose it as a compile-time env var.
fn extract_adk_version() {
    let adk_cargo = std::path::Path::new("../adk-rust/Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(adk_cargo) {
        for line in content.lines() {
            if line.starts_with("version") {
                if let Some(ver) = line.split('"').nth(1) {
                    println!("cargo:rustc-env=ADK_RUST_VERSION={}", ver);
                    return;
                }
            }
        }
    }
    println!("cargo:rustc-env=ADK_RUST_VERSION=unknown");
}

fn build_ui() {
    let ui_dir = std::path::Path::new("ui");

    if !ui_dir.exists() {
        eprintln!("cargo:warning=ui/ directory not found. UI will not be embedded.");
        ensure_dist_exists();
        return;
    }

    // Check if node_modules exists, run npm install if not
    if !ui_dir.join("node_modules").exists() {
        println!("cargo:warning=Running npm install...");
        let status = Command::new("npm")
            .arg("install")
            .current_dir(ui_dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("cargo:warning=npm install failed (status: {})", s);
                ensure_dist_exists();
                return;
            }
            Err(e) => {
                eprintln!("cargo:warning=npm not found: {}. Install Node.js 18+.", e);
                ensure_dist_exists();
                return;
            }
        }
    }

    // Always run npm run build
    println!("cargo:warning=Building React UI...");
    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(ui_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=UI built successfully.");
        }
        Ok(s) => {
            eprintln!("cargo:warning=npm run build failed (status: {})", s);
            ensure_dist_exists();
        }
        Err(e) => {
            eprintln!("cargo:warning=Failed to run npm build: {}", e);
            ensure_dist_exists();
        }
    }
}

/// Ensure ui/dist/ exists with at least a placeholder so rust-embed doesn't fail.
fn ensure_dist_exists() {
    let dist = std::path::Path::new("ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(dist).ok();
        std::fs::write(
            dist.join("index.html"),
            "<!DOCTYPE html><html><body><h1>UI not built</h1><p>Run: cd ui && npm install && npm run build</p></body></html>",
        ).ok();
    }
}

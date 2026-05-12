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

    build_ui();
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

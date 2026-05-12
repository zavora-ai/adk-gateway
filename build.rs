//! Build script — builds the React UI before compiling the gateway binary.
//!
//! This ensures `ui/dist/` exists for rust-embed to embed into the binary.
//! Skips the UI build if `ui/dist/index.html` already exists (for faster rebuilds).

use std::process::Command;

fn main() {
    // Only rebuild UI if dist doesn't exist or source changed
    let dist_index = std::path::Path::new("ui/dist/index.html");

    if !dist_index.exists() {
        println!("cargo:warning=Building React UI (ui/dist/ not found)...");
        build_ui();
    } else {
        // Check if any UI source file is newer than the dist
        println!("cargo:rerun-if-changed=ui/src/");
        println!("cargo:rerun-if-changed=ui/index.html");
        println!("cargo:rerun-if-changed=ui/package.json");
    }
}

fn build_ui() {
    let ui_dir = std::path::Path::new("ui");

    // Check if node_modules exists, run npm install if not
    if !ui_dir.join("node_modules").exists() {
        let status = Command::new("npm")
            .arg("install")
            .current_dir(ui_dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("cargo:warning=npm install failed with status: {}", s);
                eprintln!("cargo:warning=UI will not be embedded. Install Node.js 18+ and run: cd ui && npm install && npm run build");
                return;
            }
            Err(e) => {
                eprintln!("cargo:warning=npm not found: {}. UI will not be embedded.", e);
                eprintln!("cargo:warning=Install Node.js 18+ and run: cd ui && npm install && npm run build");
                // Create empty dist so rust-embed doesn't fail
                std::fs::create_dir_all("ui/dist").ok();
                std::fs::write("ui/dist/index.html", "<html><body><h1>UI not built</h1><p>Run: cd ui && npm install && npm run build</p></body></html>").ok();
                return;
            }
        }
    }

    // Run npm run build
    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(ui_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=React UI built successfully");
        }
        Ok(s) => {
            eprintln!("cargo:warning=npm run build failed with status: {}", s);
            eprintln!("cargo:warning=UI may not be embedded correctly");
        }
        Err(e) => {
            eprintln!("cargo:warning=Failed to run npm build: {}", e);
        }
    }
}

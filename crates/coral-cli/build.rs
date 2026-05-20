//! Build hints for optional CLI assets and embedded version metadata.

#![allow(
    clippy::disallowed_methods,
    clippy::print_stdout,
    reason = "Cargo build scripts read build-time environment variables directly."
)]

use std::process::Command;
use std::{
    env,
    io::ErrorKind,
    path::{Path, PathBuf},
};

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |out| String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        );
    println!("cargo:rustc-env=CORAL_GIT_SHA={sha}");

    // Trigger rebuilds when HEAD or the checked-out branch's ref moves so the
    // embedded SHA stays current.
    if let Some(head_path) = git_path("HEAD") {
        println!("cargo:rerun-if-changed={head_path}");
        if let Ok(head) = std::fs::read_to_string(&head_path)
            && let Some(reference) = head.trim().strip_prefix("ref: ")
            && let Some(reference_path) = git_path(reference)
            && std::path::Path::new(&reference_path).exists()
        {
            println!("cargo:rerun-if-changed={reference_path}");
        }
    }
    if let Some(packed_refs_path) = git_path("packed-refs")
        && std::path::Path::new(&packed_refs_path).exists()
    {
        println!("cargo:rerun-if-changed={packed_refs_path}");
    }

    if env::var_os("CARGO_FEATURE_EMBEDDED_UI").is_some() {
        let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
        let ui_dir = manifest_dir.join("../../ui");
        emit_ui_rerun_hints(&ui_dir);
        build_embedded_ui(&ui_dir);
    }
}

fn git_path(path: &str) -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--git-path", path])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|path| !path.is_empty())
}

fn emit_ui_rerun_hints(ui_dir: &Path) {
    println!("cargo:rerun-if-env-changed=PATH");
    for path in [
        "buf.gen.yaml",
        "index.html",
        "package-lock.json",
        "package.json",
        "tsconfig.json",
        "vite.config.ts",
        "public",
        "src/App.tsx",
        "src/app.css.ts",
        "src/components",
        "src/index.css",
        "src/lib",
        "src/main.tsx",
        "src/styles",
        "src/utils",
        "src/views",
        "src/wax",
        "../crates/coral-api/proto",
    ] {
        println!("cargo:rerun-if-changed={}", ui_dir.join(path).display());
    }
}

fn build_embedded_ui(ui_dir: &Path) {
    if !ui_dir.join("package.json").exists() {
        fail_build(format!(
            "embedded-ui is enabled, but the UI package was not found at {}",
            ui_dir.display()
        ));
    }

    require_tool("node", "Node.js");
    require_tool("npm", "npm");

    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(ui_dir)
        .status()
        .unwrap_or_else(|error| {
            fail_build(format!(
                "failed to start `npm run build` in {}: {error}",
                ui_dir.display()
            ));
        });

    if !status.success() {
        fail_build(
            "`npm run build` failed while preparing the embedded Coral UI.\n\
             Run `npm ci --prefix ui` from the repository root, then retry. \
             To compile without the UI, pass `--no-default-features`.",
        );
    }
}

fn require_tool(command: &str, display_name: &str) {
    let output = Command::new(command)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| match error.kind() {
            ErrorKind::NotFound => fail_build(format!(
                "embedded-ui is enabled by default, but {display_name} (`{command}`) \
                 was not found in PATH.\nInstall Node.js/npm, run \
                 `npm ci --prefix ui`, then retry. To compile without the UI, \
                 pass `--no-default-features`."
            )),
            _ => fail_build(format!(
                "failed to run `{command} --version` while preparing the embedded Coral UI: {error}"
            )),
        });

    if !output.status.success() {
        fail_build(format!(
            "`{command} --version` failed while preparing the embedded Coral UI.\n{}",
            command_output_summary(&output)
        ));
    }
}

fn command_output_summary(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut summary = format!("status: {}", output.status);
    if !stdout.trim().is_empty() {
        summary.push_str("\nstdout:\n");
        summary.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        summary.push_str("\nstderr:\n");
        summary.push_str(stderr.trim());
    }
    summary
}

fn fail_build(message: impl AsRef<str>) -> ! {
    for line in message.as_ref().lines() {
        println!("cargo::error={line}");
    }
    std::process::exit(1);
}

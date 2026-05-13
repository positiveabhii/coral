//! Build hints for optional CLI assets and embedded version metadata.

#![allow(
    clippy::disallowed_methods,
    clippy::print_stdout,
    reason = "Cargo build scripts read build-time environment variables directly."
)]

use std::process::Command;

fn main() {
    let package_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".to_owned());
    let sha = git_output(["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=CORAL_GIT_SHA={sha}");
    let mut long_version = format!("{package_version}+{sha}");

    if profile == "debug" {
        let source_path = repo_root().unwrap_or_else(|| "unknown".to_owned());
        let (tree, is_wip) = wip_tree().unwrap_or_else(|| ("unknown".to_owned(), false));
        let version_tree = if is_wip {
            format!("{tree}-wip")
        } else {
            tree.clone()
        };
        long_version = format!("{long_version} (debug, tree: {version_tree}, src: {source_path})");
        println!("cargo:rustc-env=CORAL_WIP_TREE={tree}");
        println!("cargo:rustc-env=CORAL_SOURCE_PATH={source_path}");
        println!("cargo:rustc-env=CORAL_BUILD_PROFILE=debug");
    }
    println!("cargo:rustc-env=CORAL_LONG_VERSION={long_version}");

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

    if std::env::var_os("CARGO_FEATURE_EMBEDDED_UI").is_some() {
        println!("cargo:rerun-if-changed=../../ui/dist");
        println!("cargo:rerun-if-changed=../../ui/dist/index.html");
    }
}

fn git_path(path: &str) -> Option<String> {
    git_output(["rev-parse", "--git-path", path]).filter(|path| !path.is_empty())
}

fn git_output<const N: usize>(args: [&str; N]) -> Option<String> {
    git_optional_output(args).ok().flatten()
}

fn git_optional_output<const N: usize>(args: [&str; N]) -> Result<Option<String>, ()> {
    let out = Command::new("git").args(args).output().map_err(drop)?;
    if out.status.success() {
        let output = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Ok((!output.is_empty()).then_some(output))
    } else {
        Err(())
    }
}

fn repo_root() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    std::path::Path::new(&manifest_dir)
        .parent()
        .and_then(std::path::Path::parent)
        .map(|path| path.display().to_string())
}

fn wip_tree() -> Option<(String, bool)> {
    match git_optional_output(["stash", "create"]) {
        Ok(Some(stash_commit)) => {
            return git_output(["rev-parse", "--short", &format!("{stash_commit}^{{tree}}")])
                .map(|tree| (tree, true));
        }
        Ok(None) => {}
        Err(()) => {
            if has_uncommitted_changes().unwrap_or(false) {
                return Some(("unknown".to_owned(), true));
            }
        }
    }
    git_output(["rev-parse", "--short", "HEAD^{tree}"]).map(|tree| (tree, false))
}

fn has_uncommitted_changes() -> Option<bool> {
    Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .status()
        .ok()
        .and_then(|status| match status.code() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        })
}

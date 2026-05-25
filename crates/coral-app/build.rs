//! Build script for bundled source manifests + community source catalog.

#![allow(
    clippy::disallowed_methods,
    reason = "Cargo build scripts read build-time environment variables directly."
)]

use serde_yaml::Value;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let sources_root = manifest_dir.join("../../sources");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("out dir"));

    generate_bundled(&sources_root.join("core"), &out_dir);
    generate_community_catalog(&sources_root.join("community"), &out_dir);
    emit_build_commit_sha();
}

fn generate_bundled(bundled_root: &Path, out_dir: &Path) {
    println!("cargo:rerun-if-changed={}", bundled_root.display());

    let entries = collect_source_entries(bundled_root);
    let mut generated = String::from("pub(crate) const BUNDLED_SOURCES: &[(&str, &str)] = &[\n");
    for (name, manifest_path) in entries {
        let raw = fs::read_to_string(&manifest_path).expect("read bundled manifest");
        let manifest_name = manifest_name(&raw).unwrap_or_else(|| {
            panic!(
                "bundled source '{}' is missing a top-level string name",
                manifest_path.display()
            )
        });
        assert_eq!(
            manifest_name, name,
            "bundled source directory '{name}' must match manifest name '{manifest_name}'"
        );
        writeln!(generated, "    ({name:?}, {raw:?}),").expect("writing to String is infallible");
    }
    generated.push_str("];\n");
    fs::write(out_dir.join("bundled_sources.rs"), generated).expect("write bundled source table");
}

fn generate_community_catalog(community_root: &Path, out_dir: &Path) {
    println!("cargo:rerun-if-changed={}", community_root.display());

    let entries = collect_source_entries(community_root);
    let mut generated = String::from(
        "pub(crate) const COMMUNITY_CATALOG: &[CommunityCatalogEntry] = &[\n",
    );
    for (name, manifest_path) in entries {
        let raw = fs::read_to_string(&manifest_path).expect("read community manifest");
        let manifest_name = manifest_name(&raw).unwrap_or_else(|| {
            panic!(
                "community source '{}' is missing a top-level string name",
                manifest_path.display()
            )
        });
        assert_eq!(
            manifest_name, name,
            "community source directory '{name}' must match manifest name '{manifest_name}'"
        );
        let version = manifest_string_field(&raw, "version").unwrap_or_default();
        let description = manifest_string_field(&raw, "description").unwrap_or_default();
        // Path is relative to the workspace root so it can be resolved either
        // locally (development) or via raw GitHub at the build commit SHA.
        let relative_manifest_path = format!("sources/community/{name}/manifest.yaml");
        writeln!(
            generated,
            "    CommunityCatalogEntry {{ name: {name:?}, version: {version:?}, description: {description:?}, manifest_path: {relative_manifest_path:?} }},"
        )
        .expect("writing to String is infallible");
    }
    generated.push_str("];\n");
    fs::write(out_dir.join("community_catalog.rs"), generated)
        .expect("write community catalog table");
}

fn emit_build_commit_sha() {
    // CORAL_BUILD_COMMIT_SHA pins the community manifest fetch URL in release
    // builds. Empty in dev builds; the runtime then resolves manifests from the
    // local workspace tree (CORAL_DEV_WORKSPACE_ROOT) instead of the network.
    let sha = env::var("CORAL_BUILD_COMMIT_SHA").unwrap_or_default();
    println!("cargo:rerun-if-env-changed=CORAL_BUILD_COMMIT_SHA");
    println!("cargo:rustc-env=CORAL_BUILD_COMMIT_SHA={sha}");

    let workspace_root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize workspace root");
    println!(
        "cargo:rustc-env=CORAL_DEV_WORKSPACE_ROOT={}",
        workspace_root.display()
    );
}

fn collect_source_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let mut entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read sources dir {}: {error}", root.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let manifest_path = find_manifest_file(&entry.path()).unwrap_or_else(|| {
                panic!(
                    "missing manifest.y*ml for source '{}'",
                    entry.path().display()
                )
            });
            (name, manifest_path)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn find_manifest_file(dir: &Path) -> Option<PathBuf> {
    ["manifest.yaml", "manifest.yml"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
}

fn manifest_name(raw: &str) -> Option<String> {
    manifest_string_field(raw, "name")
}

fn manifest_string_field(raw: &str, field: &str) -> Option<String> {
    let root: Value = serde_yaml::from_str(raw).ok()?;
    root.get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

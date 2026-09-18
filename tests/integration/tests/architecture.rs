//! Enforces the architectural boundaries between Argus crates.
//!
//! The allowed dependency graph is documented in the repository README. A new
//! crate must be added to [`ALLOWED`] before it can be part of the workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// For every workspace crate: the internal crates it may depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    ("argus-protocol", &[]),
    ("argus-capture", &["argus-protocol"]),
    ("argus-accessibility", &["argus-protocol"]),
    ("argus-perception", &["argus-protocol"]),
    ("argus-fusion", &["argus-protocol"]),
    ("argus-tracking", &["argus-protocol"]),
    (
        "argus-core",
        &[
            "argus-protocol",
            "argus-capture",
            "argus-accessibility",
            "argus-perception",
            "argus-fusion",
            "argus-tracking",
        ],
    ),
    ("argus-server", &["argus-protocol", "argus-core"]),
    ("argus-cli", &["argus-protocol", "argus-core", "argus-server"]),
    ("argus-integration-tests", &[]),
];

/// Internal (non-dev) dependencies of every workspace crate.
fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let members: BTreeSet<String> =
        packages.iter().map(|p| p["name"].as_str().unwrap().to_owned()).collect();

    packages
        .iter()
        .map(|package| {
            let deps = package["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|dep| dep["kind"].is_null()) // normal dependencies only
                .map(|dep| dep["name"].as_str().unwrap().to_owned())
                .filter(|name| members.contains(name))
                .collect();
            (package["name"].as_str().unwrap().to_owned(), deps)
        })
        .collect()
}

#[test]
fn every_crate_has_declared_boundaries() {
    let allowed: BTreeSet<&str> = ALLOWED.iter().map(|(name, _)| *name).collect();
    for name in workspace_graph().keys() {
        assert!(
            allowed.contains(name.as_str()),
            "crate `{name}` has no declared dependency boundaries in ALLOWED"
        );
    }
}

#[test]
fn crates_depend_only_on_allowed_crates() {
    let allowed: BTreeMap<&str, &[&str]> = ALLOWED.iter().copied().collect();
    for (name, deps) in workspace_graph() {
        let Some(permitted) = allowed.get(name.as_str()) else {
            continue; // reported by `every_crate_has_declared_boundaries`
        };
        for dep in deps {
            assert!(permitted.contains(&dep.as_str()), "`{name}` must not depend on `{dep}`");
        }
    }
}

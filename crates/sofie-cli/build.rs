//! Records where the CLI was built from, so `sofie new app` names the framework by version or
//! commit rather than by a path into cargo's cache, which it may clean.

use std::fs;
use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = Path::new(&manifest);
    // Cargo marks a package it unpacked from a registry, and a git checkout it made, with this
    // file; a developer's own clone has none.
    if manifest.join(".cargo-ok").exists() {
        println!("cargo::rustc-env=SOFIE_CLI_REGISTRY=1");
        return;
    }
    let Some(root) = manifest.ancestors().nth(2) else {
        return;
    };
    if !root.join(".cargo-ok").exists() {
        return;
    }
    if let Some(commit) = head_commit(&root.join(".git")) {
        println!("cargo::rustc-env=SOFIE_CLI_COMMIT={commit}");
    }
}

/// Read from the files, as cargo may have checked out without a `git` program installed.
fn head_commit(git: &Path) -> Option<String> {
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        return is_commit(head).then(|| head.to_owned());
    };
    if let Ok(commit) = fs::read_to_string(git.join(reference)) {
        return Some(commit.trim().to_owned()).filter(|commit| is_commit(commit));
    }
    let packed = fs::read_to_string(git.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (commit, name) = line.split_once(' ')?;
        (name == reference && is_commit(commit)).then(|| commit.to_owned())
    })
}

fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

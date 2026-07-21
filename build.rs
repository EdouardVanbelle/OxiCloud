//! build.rs — injects git build metadata into the binary.
//!
//! Exposes `GIT_HASH` and `GIT_BRANCH` (consumed via `env!()` in `main.rs`).
//! There is no Rust-side asset pipeline: the frontend is built by Vite into
//! `static-dist/` and served directly by the web layer (`interfaces::web`).

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    git_status();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Grab git values
// Supports GitHub; CI vars are honoured (extend if moving to GitLab/CircleCI/…).
// ═══════════════════════════════════════════════════════════════════════════════
fn git_status() {
    // Rerun the build script when the commit or branch changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");

    let git_hash = first_env(&["GITHUB_SHA", "CI_COMMIT_SHA", "CIRCLE_SHA1", "GIT_COMMIT"])
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=GIT_HASH={git_hash}");

    let git_branch = first_env(&[
        "GITHUB_HEAD_REF",    // GitHub: PR source branch (empty on push)
        "GITHUB_REF_NAME",    // GitHub: branch/tag on push
        "CI_COMMIT_REF_NAME", // GitLab
        "CIRCLE_BRANCH",      // CircleCI
        "GIT_BRANCH",         // Jenkins
    ])
    .or_else(|| git(&["rev-parse", "--abbrev-ref", "HEAD"]))
    .filter(|b| b != "HEAD") // detached HEAD is not a real branch name
    .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_BRANCH={git_branch}");

    // CI builds: rerun if the injected env changes
    for k in [
        "GITHUB_SHA",
        "GITHUB_HEAD_REF",
        "GITHUB_REF_NAME",
        "CI_COMMIT_SHA",
        "CI_COMMIT_REF_NAME",
        "CIRCLE_SHA1",
        "CIRCLE_BRANCH",
        "GIT_COMMIT",
        "GIT_BRANCH",
    ] {
        println!("cargo:rerun-if-env-changed={k}");
    }

    println!("cargo:warning=OxiCloud built with git hash: {git_hash} and branch: {git_branch}");
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then_some(())?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| env::var(k).ok())
        .filter(|s| !s.is_empty())
}

//! Scratch git repositories for tests.
//!
//! Shared by the unit-test modules (`crate::test_support`) and by the
//! integration tests, which include this file with `#[path]` because
//! `#[cfg(test)]` items in the library are not visible to `tests/`. It must
//! therefore depend on `std` only.
//!
//! Every command runs with the global and system git config disabled and a
//! fixed identity, so a developer's own settings (signing, hooks, colour) never
//! leak into a test.

use std::path::Path;
use std::process::Command;

/// A `git` command in `repo` that ignores global and system config.
pub fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Annatar Test")
        .env("GIT_AUTHOR_EMAIL", "annatar@test.invalid")
        .env("GIT_COMMITTER_NAME", "Annatar Test")
        .env("GIT_COMMITTER_EMAIL", "annatar@test.invalid");
    command
}

/// Run `git args` in `repo` and panic with git's stderr if it fails.
pub fn git_ok(repo: &Path, args: &[&str]) {
    let output = git(repo).args(args).output().expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `git init` with a local identity and signing off.
pub fn init_repo(repo: &Path) {
    git_ok(repo, &["init", "-q"]);
    git_ok(repo, &["config", "user.email", "annatar@test.invalid"]);
    git_ok(repo, &["config", "user.name", "Annatar Test"]);
    git_ok(repo, &["config", "commit.gpgsign", "false"]);
}

/// Stage everything and commit at `date`, returning the new commit's full sha.
pub fn commit(repo: &Path, message: &str, date: &str) -> String {
    git_ok(repo, &["add", "-A"]);
    let output = git(repo)
        .args(["commit", "-q", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = git(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git should run");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

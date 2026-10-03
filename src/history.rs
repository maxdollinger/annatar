//! Commits that touched a line range.
//!
//! [`history_for_span`] shells out to `git log -L <start>,<end>:<file>` and
//! parses only the format records, ignoring the diff hunks git prints after
//! each commit. The format separates fields with `%x1f`, records with `%x1e`,
//! and terminates each record with `%x1d`; those control characters do not
//! occur in ordinary commit text, so a subject or body can never split a
//! record.
//!
//! Commits come back newest first. `git log` walks history in reverse
//! chronological order, so the first element is the most recent change and the
//! last element is the commit that introduced the lines.
//!
//! The commit that created the lines is part of the result: `-L` reports it as
//! the last record because adding a line is a change to it.
//!
//! # Renames
//!
//! `git log -L` follows renames through git's own rename detection (the same
//! machinery as `git blame`): pointing it at a file that was renamed still
//! walks back through the pre-rename history. `--follow` cannot be combined
//! with `-L`; git rejects it with `fatal: --follow requires exactly one
//! pathspec`, and it is unnecessary because `-L` already follows.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// Start of a format record (ASCII record separator).
const RECORD_START: char = '\u{1e}';
/// Separates the fields within a record (ASCII unit separator).
const FIELD_SEP: char = '\u{1f}';
/// Ends a format record (ASCII group separator).
const RECORD_END: char = '\u{1d}';

/// Git format: record start, sha, author date, subject, body, record end.
/// The trailing `%x1d` lets the parser drop the diff hunk that follows.
const GIT_FORMAT: &str = "%x1e%H%x1f%aI%x1f%s%x1f%b%x1d";

/// One commit that changed a line span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full commit hash (`git log`'s `%H`).
    pub sha: String,
    /// Author date in strict ISO 8601 (`%aI`), e.g. `2024-01-31T09:15:00+01:00`.
    pub date: String,
    /// First line of the commit message (`%s`).
    pub subject: String,
    /// Commit message body, without the subject and without git's trailing
    /// newline (`%b`).
    pub body: String,
}

/// The commits that touched `file`'s inclusive, 1-based line range
/// `start_line..=end_line`, newest first.
///
/// `repo` is the repository root and `file` is a path relative to it. The
/// commit that introduced the lines is included, as the last element. A
/// non-zero `git log` exit is an error carrying git's stderr: an untracked
/// file, a range past the end of the file, and a non-repository each fail
/// loudly rather than yielding an empty list.
pub fn history_for_span(
    repo: &Path,
    file: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<Commit>> {
    let range = format!("{},{}:{}", start_line, end_line, file.display());
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("log")
        .arg("-L")
        .arg(&range)
        .arg(format!("--format={GIT_FORMAT}"))
        .output()
        .with_context(|| format!("running git log -L {range} in {}", repo.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git log -L {range} failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_records(&stdout))
}

/// Extract the format records from `git log -L` output, skipping the diff
/// hunks that follow each record. Text outside a record pair is ignored.
fn parse_records(stdout: &str) -> Vec<Commit> {
    stdout
        .split(RECORD_START)
        .skip(1)
        .filter_map(|chunk| {
            let record = chunk.split(RECORD_END).next()?;
            let mut fields = record.splitn(4, FIELD_SEP);
            let sha = fields.next()?;
            let date = fields.next()?;
            let subject = fields.next()?;
            let body = fields.next().unwrap_or_default();
            if sha.is_empty() {
                return None;
            }
            Some(Commit {
                sha: sha.to_string(),
                date: date.to_string(),
                subject: subject.to_string(),
                body: body.trim_end_matches('\n').to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path) -> Command {
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

    fn git_ok(repo: &Path, args: &[&str]) {
        let output = git(repo).args(args).output().expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(repo: &Path) {
        git_ok(repo, &["init", "-q"]);
        git_ok(repo, &["config", "user.email", "annatar@test.invalid"]);
        git_ok(repo, &["config", "user.name", "Annatar Test"]);
        git_ok(repo, &["config", "commit.gpgsign", "false"]);
    }

    /// Stage everything and commit, returning the new commit's full sha.
    fn commit(repo: &Path, message: &str, date: &str) -> String {
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

    fn write(repo: &Path, contents: &str) {
        std::fs::write(repo.join("f.txt"), contents).unwrap();
    }

    fn subjects(commits: &[Commit]) -> Vec<&str> {
        commits
            .iter()
            .map(|commit| commit.subject.as_str())
            .collect()
    }

    /// A repo where `create` added five lines, then two later commits changed
    /// line 2 and line 5 respectively. Newest first, that is C, B, A.
    fn repo_with_three_commits() -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        write(repo, "a\nb\nc\nd\ne\n");
        let a = commit(repo, "create", "2024-01-01T00:00:00+00:00");
        write(repo, "a\nB2\nc\nd\ne\n");
        let b = commit(repo, "change line2", "2024-02-02T00:00:00+00:00");
        write(repo, "a\nB2\nc\nd\nE5\n");
        let c = commit(repo, "change line5", "2024-03-03T00:00:00+00:00");
        (dir, a, b, c)
    }

    #[test]
    fn span_touched_by_two_commits_returns_each_with_the_introducing_commit() {
        let (dir, a, b, c) = repo_with_three_commits();
        let file = Path::new("f.txt");

        let line2 = history_for_span(dir.path(), file, 2, 2).unwrap();
        assert_eq!(subjects(&line2), ["change line2", "create"]);
        assert_eq!(
            line2
                .iter()
                .map(|commit| commit.sha.as_str())
                .collect::<Vec<_>>(),
            [b.as_str(), a.as_str()],
            "the change comes first, then the commit that added the line"
        );

        let line5 = history_for_span(dir.path(), file, 5, 5).unwrap();
        assert_eq!(subjects(&line5), ["change line5", "create"]);
        assert_eq!(
            line5
                .iter()
                .map(|commit| commit.sha.as_str())
                .collect::<Vec<_>>(),
            [c.as_str(), a.as_str()]
        );
    }

    #[test]
    fn span_changed_by_only_the_introducing_commit_returns_only_that_commit() {
        let (dir, a, _, _) = repo_with_three_commits();

        let untouched = history_for_span(dir.path(), Path::new("f.txt"), 4, 4).unwrap();

        assert_eq!(subjects(&untouched), ["create"]);
        assert_eq!(untouched[0].sha, a);
    }

    #[test]
    fn commit_fields_include_date_and_multi_line_body() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        write(repo, "one\ntwo\n");
        let sha = commit(
            repo,
            "add greeting\n\ndescribe the greeting\nsecond body line\n\ncloses GRLD-1",
            "2024-04-05T06:07:08+02:00",
        );

        let commits = history_for_span(repo, Path::new("f.txt"), 1, 1).unwrap();

        assert_eq!(commits.len(), 1);
        let commit = &commits[0];
        assert_eq!(commit.sha, sha);
        assert_eq!(commit.subject, "add greeting");
        assert_eq!(commit.date, "2024-04-05T06:07:08+02:00");
        assert_eq!(
            commit.body, "describe the greeting\nsecond body line\n\ncloses GRLD-1",
            "the body keeps its paragraphs and trailing whitespace is stripped"
        );
    }

    #[test]
    fn file_without_history_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        std::fs::write(repo.join("other.txt"), "tracked\n").unwrap();
        commit(repo, "add tracked file", "2024-01-01T00:00:00+00:00");
        std::fs::write(repo.join("untracked.txt"), "never committed\n").unwrap();

        let err = history_for_span(repo, Path::new("untracked.txt"), 1, 1)
            .expect_err("an untracked file has no history and should fail");

        let message = format!("{err:#}");
        assert!(
            message.contains("untracked.txt"),
            "error should name the file, got: {message}"
        );
    }

    #[test]
    fn not_a_repository_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x\n").unwrap();

        let err = history_for_span(dir.path(), Path::new("f.txt"), 1, 1)
            .expect_err("a plain directory is not a git repository");

        assert!(
            format!("{err:#}").contains("git log -L"),
            "error should carry the git command, got: {err:#}"
        );
    }

    #[test]
    fn range_past_the_end_of_the_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        write(repo, "one\ntwo\n");
        commit(repo, "create", "2024-01-01T00:00:00+00:00");

        assert!(
            history_for_span(repo, Path::new("f.txt"), 99, 99).is_err(),
            "a range past the end of the file should fail"
        );
    }
}

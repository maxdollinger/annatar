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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use libsql::{Connection, params};
use regex::Regex;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Whether `repo` is (inside) a git work tree, checked once per index run so
/// the caller can degrade to a structure-only index instead of failing.
///
/// Any failure to run `git` at all (not installed, not a repo) is reported as
/// `false`, never as an error: the caller only uses this to decide whether to
/// look for history, and a missing `git` is itself a reason not to.
pub fn is_repository(repo: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The full sha of the most recent commit that touched `file`, or `None` when
/// no commit has (an untracked file).
///
/// One `git log -1` per file is the cheap half of the history cache's key: a
/// `None` means there is nothing to key on, so the caller skips the cache and
/// falls back to per-symbol history. A non-zero git exit is an error, never a
/// silent `None`.
pub fn file_last_commit(repo: &Path, file: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("log")
        .arg("-1")
        .arg("--format=%H")
        .arg("--")
        .arg(file)
        .output()
        .with_context(|| format!("running git log -1 for {}", file.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git log -1 for {} failed: {}",
            file.display(),
            stderr.trim()
        );
    }

    let sha = String::from_utf8_lossy(&output.stdout);
    let sha = sha.trim();
    if sha.is_empty() {
        Ok(None)
    } else {
        Ok(Some(sha.to_string()))
    }
}

/// The files under `repo` whose working copy differs from `HEAD`, staged or
/// not, as paths relative to `repo`.
///
/// Spans come from the working copy while `git log -L` resolves them against
/// `HEAD`, so a dirty file's history can be attributed to the wrong lines. The
/// caller uses this set, computed once per run, to keep such history out of the
/// cache. Untracked files are not listed; they have no history at all. A
/// non-zero git exit (no `HEAD` yet, say) is an error.
pub fn dirty_files(repo: &Path) -> Result<HashSet<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "HEAD",
            "--name-only",
            "-z",
            "--relative",
            "--no-renames",
        ])
        .output()
        .with_context(|| format!("running git diff HEAD in {}", repo.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff HEAD failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// The persistent history cache in `cache.db`.
///
/// A symbol's commits are keyed by its fqn plus the content hash and the last
/// commit sha of its file, so a repeat run over unchanged sources reuses the
/// stored commits without shelling out to `git log -L`. A miss or an outdated
/// row is refilled by the caller. Writes go to the cache connection, never to
/// the index build, so an aborted index cannot roll them back.
pub struct HistoryCache<'a> {
    conn: &'a Connection,
}

impl<'a> HistoryCache<'a> {
    /// Wrap a cache connection (typically [`crate::store::Store::cache`]).
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// The cached commits for `fqn`, if the stored key still matches.
    pub async fn get(
        &self,
        fqn: &str,
        content_hash: &str,
        file_last_commit: &str,
    ) -> Result<Option<Vec<Commit>>> {
        let mut rows = self
            .conn
            .query(
                "SELECT content_hash, file_last_commit, commits
                 FROM history_cache_v2 WHERE fqn = ?1",
                params![fqn],
            )
            .await
            .context("reading history cache")?;
        let Some(row) = rows.next().await.context("reading history cache row")? else {
            return Ok(None);
        };
        let stored_hash = row
            .get::<String>(0)
            .context("reading history_cache_v2.content_hash")?;
        let stored_commit = row
            .get::<String>(1)
            .context("reading history_cache_v2.file_last_commit")?;
        if stored_hash != content_hash || stored_commit != file_last_commit {
            return Ok(None);
        }
        let stored = row
            .get::<String>(2)
            .context("reading history_cache_v2.commits")?;
        let commits = serde_json::from_str(&stored).context("decoding cached commits")?;
        Ok(Some(commits))
    }

    /// Store `commits` for `fqn`, replacing any previous row for that fqn.
    pub async fn put(
        &self,
        fqn: &str,
        content_hash: &str,
        file_last_commit: &str,
        commits: &[Commit],
    ) -> Result<()> {
        let stored = serde_json::to_string(commits).context("encoding cached commits")?;
        self.conn
            .execute(
                "INSERT INTO history_cache_v2 (fqn, content_hash, file_last_commit, commits)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(fqn) DO UPDATE SET
                     content_hash = excluded.content_hash,
                     file_last_commit = excluded.file_last_commit,
                     commits = excluded.commits",
                params![fqn, content_hash, file_last_commit, stored],
            )
            .await
            .with_context(|| format!("writing history cache for {fqn}"))?;
        Ok(())
    }
}

/// The distinct ticket keys `regex` finds in `text`, in first-seen order.
///
/// Zero-width matches are ignored: an empty pattern (an empty `ticket_regex`
/// in config) matches at every position with width zero, and yielding empty
/// keys would pollute `symbol_tickets`. Duplicates are dropped so one text
/// yields each key once.
pub fn ticket_keys(regex: &Regex, text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for matched in regex.find_iter(text) {
        let key = matched.as_str();
        if key.is_empty() || keys.iter().any(|seen| seen == key) {
            continue;
        }
        keys.push(key.to_string());
    }
    keys
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
    use crate::test_support::{commit, git_ok, init_repo};

    #[test]
    fn dirty_files_lists_staged_and_unstaged_changes_relative_to_repo() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let repo = root.path().join("module");
        for file in ["a.txt", "b.txt", "clean.txt"] {
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::write(repo.join(file), "1\n").unwrap();
        }
        std::fs::write(root.path().join("outside.txt"), "1\n").unwrap();
        commit(root.path(), "initial", "2024-01-01T00:00:00+00:00");
        std::fs::write(repo.join("a.txt"), "2\n").unwrap();
        std::fs::write(repo.join("b.txt"), "2\n").unwrap();
        git_ok(root.path(), &["add", "module/b.txt"]);
        std::fs::write(repo.join("untracked.txt"), "1\n").unwrap();
        std::fs::write(root.path().join("outside.txt"), "2\n").unwrap();

        let dirty = dirty_files(&repo).unwrap();

        assert_eq!(
            dirty,
            HashSet::from([PathBuf::from("a.txt"), PathBuf::from("b.txt")]),
            "unstaged and staged edits, relative to `repo`, nothing outside it"
        );
    }

    #[test]
    fn dirty_files_is_empty_for_a_clean_tree() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "1\n");
        commit(repo.path(), "initial", "2024-01-01T00:00:00+00:00");

        assert!(dirty_files(repo.path()).unwrap().is_empty());
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

    #[test]
    fn is_repository_distinguishes_a_plain_directory_from_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !is_repository(dir.path()),
            "a plain directory is not a git work tree"
        );

        init_repo(dir.path());
        assert!(
            is_repository(dir.path()),
            "an initialized repo should be detected"
        );
    }

    #[test]
    fn ticket_keys_finds_distinct_keys_and_drops_duplicates() {
        let regex = Regex::new(crate::config::DEFAULT_TICKET_REGEX).unwrap();

        assert_eq!(
            ticket_keys(&regex, "GRLD-1 add, GRLD-2 refine GRLD-1"),
            vec!["GRLD-1".to_string(), "GRLD-2".to_string()],
            "keys are deduplicated in first-seen order"
        );
        assert!(ticket_keys(&regex, "no ticket here").is_empty());
    }

    #[test]
    fn ticket_keys_ignores_zero_width_matches() {
        let empty = Regex::new("").unwrap();
        assert!(
            ticket_keys(&empty, "abc").is_empty(),
            "an empty pattern matches everywhere with width zero and must yield no keys"
        );
        assert!(
            ticket_keys(&empty, "").is_empty(),
            "an empty pattern on empty text must also yield nothing"
        );
    }
}

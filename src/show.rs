//! Render a symbol and its descendants as text.
//!
//! [`render`] looks a symbol up by its fully qualified name and prints its
//! stored fields, then every child — nested types and members, recursively —
//! sorted by source line. This is the read side of the index: `annatar show`
//! and later the MCP `get_symbol` tool both build on it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use libsql::{Connection, Row};

/// One row of the `symbol_commits` table, as read back for display.
struct StoredCommit {
    symbol_id: i64,
    sha: String,
    date: String,
    subject: String,
}

/// One row of the `symbol_tickets` table, as read back for display.
struct StoredTicket {
    symbol_id: i64,
    ticket_key: String,
    first_date: String,
    last_date: String,
}

/// One row of the `symbols` table, as read back for display.
struct StoredSymbol {
    id: i64,
    parent_id: Option<i64>,
    kind: String,
    role: Option<String>,
    fqn: String,
    file: String,
    start_line: i64,
    end_line: i64,
    signature: String,
    javadoc: Option<String>,
    annotations: String,
}

impl StoredSymbol {
    fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            id: row.get(0).context("reading symbols.id")?,
            parent_id: row.get(1).context("reading symbols.parent_id")?,
            kind: row.get(2).context("reading symbols.kind")?,
            role: row.get(3).context("reading symbols.role")?,
            fqn: row.get(4).context("reading symbols.fqn")?,
            file: row.get(5).context("reading symbols.file")?,
            start_line: row.get(6).context("reading symbols.start_line")?,
            end_line: row.get(7).context("reading symbols.end_line")?,
            signature: row.get(8).context("reading symbols.signature")?,
            javadoc: row.get(9).context("reading symbols.javadoc")?,
            annotations: row.get(10).context("reading symbols.annotations")?,
        })
    }
}

/// Render the symbol named `fqn` and all its children as indented text.
///
/// A missing `fqn` is an error that names the symbol.
pub async fn render(conn: &Connection, fqn: &str) -> Result<String> {
    let symbols = load_all(conn).await?;
    let commits = load_commits(conn).await?;
    let tickets = load_tickets(conn).await?;
    let root = symbols
        .iter()
        .find(|symbol| symbol.fqn == fqn)
        .with_context(|| format!("symbol `{fqn}` not found in the index"))?;

    let mut children: HashMap<i64, Vec<&StoredSymbol>> = HashMap::new();
    for symbol in &symbols {
        if let Some(parent) = symbol.parent_id {
            children.entry(parent).or_default().push(symbol);
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(|symbol| (symbol.start_line, symbol.id));
    }

    let mut out = String::new();
    write_symbol(root, &children, &commits, &tickets, 0, &mut out)?;
    Ok(out)
}

async fn load_all(conn: &Connection) -> Result<Vec<StoredSymbol>> {
    let mut rows = conn
        .query(
            "SELECT id, parent_id, kind, role, fqn, file, start_line, end_line, signature, javadoc, annotations
             FROM symbols",
            (),
        )
        .await
        .context("reading symbols")?;
    let mut symbols = Vec::new();
    while let Some(row) = rows.next().await.context("reading symbols row")? {
        symbols.push(StoredSymbol::from_row(&row)?);
    }
    Ok(symbols)
}

async fn load_commits(conn: &Connection) -> Result<HashMap<i64, Vec<StoredCommit>>> {
    let mut rows = conn
        .query(
            "SELECT symbol_id, sha, date, subject FROM symbol_commits ORDER BY id",
            (),
        )
        .await
        .context("reading symbol_commits")?;
    let mut commits: HashMap<i64, Vec<StoredCommit>> = HashMap::new();
    while let Some(row) = rows.next().await.context("reading symbol_commits row")? {
        let commit = StoredCommit {
            symbol_id: row.get(0).context("reading symbol_commits.symbol_id")?,
            sha: row.get(1).context("reading symbol_commits.sha")?,
            date: row.get(2).context("reading symbol_commits.date")?,
            subject: row.get(3).context("reading symbol_commits.subject")?,
        };
        commits.entry(commit.symbol_id).or_default().push(commit);
    }
    Ok(commits)
}

async fn load_tickets(conn: &Connection) -> Result<HashMap<i64, Vec<StoredTicket>>> {
    let mut rows = conn
        .query(
            "SELECT symbol_id, ticket_key, first_date, last_date
             FROM symbol_tickets ORDER BY ticket_key",
            (),
        )
        .await
        .context("reading symbol_tickets")?;
    let mut tickets: HashMap<i64, Vec<StoredTicket>> = HashMap::new();
    while let Some(row) = rows.next().await.context("reading symbol_tickets row")? {
        let ticket = StoredTicket {
            symbol_id: row.get(0).context("reading symbol_tickets.symbol_id")?,
            ticket_key: row.get(1).context("reading symbol_tickets.ticket_key")?,
            first_date: row.get(2).context("reading symbol_tickets.first_date")?,
            last_date: row.get(3).context("reading symbol_tickets.last_date")?,
        };
        tickets.entry(ticket.symbol_id).or_default().push(ticket);
    }
    Ok(tickets)
}

fn write_symbol(
    symbol: &StoredSymbol,
    children: &HashMap<i64, Vec<&StoredSymbol>>,
    commits: &HashMap<i64, Vec<StoredCommit>>,
    tickets: &HashMap<i64, Vec<StoredTicket>>,
    depth: usize,
    out: &mut String,
) -> Result<()> {
    let indent = "  ".repeat(depth);
    let mut header = format!("{indent}{} [{}]", symbol.fqn, symbol.kind);
    if let Some(role) = &symbol.role {
        header.push_str(" role=");
        header.push_str(role);
    }
    out.push_str(&header);
    out.push('\n');

    let field = "  ".repeat(depth + 1);
    out.push_str(&format!(
        "{field}- file: {}:{}-{}\n",
        symbol.file, symbol.start_line, symbol.end_line
    ));
    out.push_str(&format!("{field}- signature: {}\n", symbol.signature));

    let annotations = parse_annotations(&symbol.annotations)
        .with_context(|| format!("parsing annotations for `{}`", symbol.fqn))?;
    if !annotations.is_empty() {
        out.push_str(&format!(
            "{field}- annotations: {}\n",
            annotations.join(", ")
        ));
    }
    if let Some(javadoc) = &symbol.javadoc {
        out.push_str(&format!("{field}- javadoc:\n"));
        for line in javadoc.lines() {
            out.push_str(&format!("{field}  {line}\n"));
        }
    }

    if let Some(list) = commits.get(&symbol.id) {
        out.push_str(&format!("{field}- commits:\n"));
        for commit in list {
            let short = short_sha(&commit.sha);
            out.push_str(&format!(
                "{field}  {short} {} {}\n",
                commit.date, commit.subject
            ));
        }
    }
    if let Some(list) = tickets.get(&symbol.id) {
        out.push_str(&format!("{field}- tickets:\n"));
        for ticket in list {
            out.push_str(&format!(
                "{field}  {} (first: {}, last: {})\n",
                ticket.ticket_key, ticket.first_date, ticket.last_date
            ));
        }
    }

    if let Some(list) = children.get(&symbol.id) {
        for child in list {
            write_symbol(child, children, commits, tickets, depth + 1, out)?;
        }
    }
    Ok(())
}

/// The first 8 characters of a sha, enough to identify a commit in the index.
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn parse_annotations(raw: &str) -> Result<Vec<String>> {
    serde_json::from_str(raw).context("decoding annotations JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_TICKET_REGEX;
    use crate::indexer::build_index;
    use crate::store::{IndexReader, Store};
    use regex::Regex;
    use std::process::Command;

    const SOURCE: &str = "\
package com.acme.show;

/** A widget. */
public class Widget {
    /** Builds one. */
    public Widget build() { return null; }

    public static class Part {
        void run() {}
    }
}
";

    async fn indexed() -> (tempfile::TempDir, tempfile::TempDir) {
        let repo = tempfile::tempdir().unwrap();
        let file = repo.path().join("src/main/java/com/acme/show/Widget.java");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, SOURCE).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let regex = Regex::new(DEFAULT_TICKET_REGEX).unwrap();
        build_index(&store, repo.path(), None, &regex)
            .await
            .unwrap();
        (repo, data)
    }

    #[tokio::test]
    async fn renders_a_symbol_and_its_children_in_source_order() {
        let (_repo, data) = indexed().await;
        let reader = IndexReader::open(data.path()).await.unwrap();

        let output = render(reader.connection(), "com.acme.show.Widget")
            .await
            .unwrap();

        let expected = "\
com.acme.show.Widget [class]
  - file: src/main/java/com/acme/show/Widget.java:4-11
  - signature: public class Widget
  - javadoc:
    A widget.
  com.acme.show.Widget#build() [method]
    - file: src/main/java/com/acme/show/Widget.java:6-6
    - signature: public Widget build()
    - javadoc:
      Builds one.
  com.acme.show.Widget.Part [class]
    - file: src/main/java/com/acme/show/Widget.java:8-10
    - signature: public static class Part
    com.acme.show.Widget.Part#run() [method]
      - file: src/main/java/com/acme/show/Widget.java:9-9
      - signature: void run()
";
        assert_eq!(output, expected);
    }

    #[tokio::test]
    async fn unknown_fqn_is_an_error_that_names_it() {
        let (_repo, data) = indexed().await;
        let reader = IndexReader::open(data.path()).await.unwrap();

        let err = render(reader.connection(), "com.acme.show.Missing")
            .await
            .expect_err("unknown fqn should fail");
        assert!(
            format!("{err:#}").contains("com.acme.show.Missing"),
            "error should name the missing fqn, got: {err:#}"
        );
    }

    #[tokio::test]
    async fn corrupt_annotations_error_and_name_the_fqn() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        build
            .connection()
            .execute(
                "INSERT INTO symbols
                    (id, parent_id, kind, fqn, file, start_line, end_line, signature, annotations, content_hash)
                 VALUES (1, NULL, 'class', 'com.acme.show.Broken', 'Broken.java', 1, 1, 'class Broken', 'not json', 'hash')",
                (),
            )
            .await
            .unwrap();
        build.commit().unwrap();

        let reader = IndexReader::open(data.path()).await.unwrap();
        let err = render(reader.connection(), "com.acme.show.Broken")
            .await
            .expect_err("corrupt annotations should fail rendering");
        assert!(
            format!("{err:#}").contains("com.acme.show.Broken"),
            "error should name the fqn, got: {err:#}"
        );
    }

    fn git(repo: &std::path::Path) -> Command {
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

    fn git_ok(repo: &std::path::Path, args: &[&str]) {
        let output = git(repo).args(args).output().expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(repo: &std::path::Path, message: &str, date: &str) {
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
    }

    #[tokio::test]
    async fn render_lists_commits_and_tickets_for_a_git_backed_symbol() {
        let repo = tempfile::tempdir().unwrap();
        git_ok(repo.path(), &["init", "-q"]);
        git_ok(
            repo.path(),
            &["config", "user.email", "annatar@test.invalid"],
        );
        git_ok(repo.path(), &["config", "user.name", "Annatar Test"]);
        git_ok(repo.path(), &["config", "commit.gpgsign", "false"]);
        let file = repo.path().join("src/main/java/com/acme/show/Widget.java");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, SOURCE).unwrap();
        commit(
            repo.path(),
            "GRLD-42 build the widget",
            "2024-01-01T00:00:00+01:00",
        );

        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let regex = Regex::new(DEFAULT_TICKET_REGEX).unwrap();
        build_index(&store, repo.path(), None, &regex)
            .await
            .unwrap();
        drop(store);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = render(reader.connection(), "com.acme.show.Widget")
            .await
            .unwrap();

        assert!(output.contains("- commits:"), "commits section: {output}");
        assert!(
            output.contains("2024-01-01T00:00:00+01:00 GRLD-42 build the widget"),
            "commit line: {output}"
        );
        assert!(output.contains("- tickets:"), "tickets section: {output}");
        assert!(
            output.contains(
                "GRLD-42 (first: 2024-01-01T00:00:00+01:00, last: 2024-01-01T00:00:00+01:00)"
            ),
            "ticket line: {output}"
        );
    }
}

//! Render a symbol and its descendants as text.
//!
//! [`render`] looks a symbol up by its fully qualified name and prints its
//! stored fields, then every child — nested types and members, recursively —
//! sorted by source line, each with the model's `what` and `why` when it has
//! them. This is the read side of the index: `annatar show`
//! and later the MCP `get_symbol` tool both build on it. Ticket types, titles
//! and the model's summary and purpose come from the index `tickets` table,
//! never from `cache.db`.

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

/// One row of the `tickets` table, as read back for display.
enum TicketInfo {
    Available {
        issue_type: String,
        summary: String,
        /// The model's summary, when the ticket has one, and its purpose
        /// (`None` when the ticket gives no reason).
        brief: Option<(String, Option<String>)>,
    },
    Unavailable,
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
    what: Option<String>,
    why: Option<String>,
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
            what: row.get(11).context("reading symbols.what")?,
            why: row.get(12).context("reading symbols.why")?,
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
    let info = load_ticket_info(conn).await?;
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
    let rows = Rows {
        children: &children,
        commits: &commits,
        tickets: &tickets,
        info: &info,
    };
    write_symbol(root, &rows, 0, &mut out)?;
    Ok(out)
}

async fn load_all(conn: &Connection) -> Result<Vec<StoredSymbol>> {
    let mut rows = conn
        .query(
            "SELECT id, parent_id, kind, role, fqn, file, start_line, end_line, signature, javadoc, annotations, what, why
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

async fn load_ticket_info(conn: &Connection) -> Result<HashMap<String, TicketInfo>> {
    let mut rows = conn
        .query(
            "SELECT key, unavailable, issue_type, summary, llm_summary, llm_purpose FROM tickets",
            (),
        )
        .await
        .context("reading tickets")?;
    let mut info = HashMap::new();
    while let Some(row) = rows.next().await.context("reading tickets row")? {
        let key: String = row.get(0).context("reading tickets.key")?;
        let unavailable: i64 = row.get(1).context("reading tickets.unavailable")?;
        let issue_type: Option<String> = row.get(2).context("reading tickets.issue_type")?;
        let summary: Option<String> = row.get(3).context("reading tickets.summary")?;
        let llm_summary: Option<String> = row.get(4).context("reading tickets.llm_summary")?;
        let llm_purpose: Option<String> = row.get(5).context("reading tickets.llm_purpose")?;
        let ticket = match (unavailable, issue_type, summary) {
            (0, Some(issue_type), Some(summary)) => TicketInfo::Available {
                issue_type,
                summary,
                brief: llm_summary.map(|summary| (summary, llm_purpose)),
            },
            (0, _, _) => anyhow::bail!("tickets row for {key} has content missing"),
            _ => TicketInfo::Unavailable,
        };
        info.insert(key, ticket);
    }
    Ok(info)
}

/// Everything loaded for rendering, keyed by symbol id (or ticket key).
struct Rows<'a> {
    children: &'a HashMap<i64, Vec<&'a StoredSymbol>>,
    commits: &'a HashMap<i64, Vec<StoredCommit>>,
    tickets: &'a HashMap<i64, Vec<StoredTicket>>,
    info: &'a HashMap<String, TicketInfo>,
}

fn write_symbol(symbol: &StoredSymbol, rows: &Rows, depth: usize, out: &mut String) -> Result<()> {
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
    if let Some(what) = &symbol.what {
        out.push_str(&format!("{field}- what: {what}\n"));
    }
    if let Some(why) = &symbol.why {
        out.push_str(&format!("{field}- why: {why}\n"));
    }

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

    if let Some(list) = rows.commits.get(&symbol.id) {
        out.push_str(&format!("{field}- commits:\n"));
        for commit in list {
            let short = short_sha(&commit.sha);
            out.push_str(&format!(
                "{field}  {short} {} {}\n",
                commit.date, commit.subject
            ));
        }
    }
    if let Some(list) = rows.tickets.get(&symbol.id) {
        out.push_str(&format!("{field}- tickets:\n"));
        for ticket in list {
            let (detail, brief) = match rows.info.get(&ticket.ticket_key) {
                Some(TicketInfo::Available {
                    issue_type,
                    summary,
                    brief,
                }) => (format!(" [{issue_type}] {summary}"), brief.as_ref()),
                Some(TicketInfo::Unavailable) => (" (unavailable)".to_string(), None),
                None => (String::new(), None),
            };
            out.push_str(&format!(
                "{field}  {} (first: {}, last: {}){detail}\n",
                ticket.ticket_key, ticket.first_date, ticket.last_date
            ));
            if let Some((summary, purpose)) = brief {
                out.push_str(&format!("{field}    summary: {summary}\n"));
                if let Some(purpose) = purpose {
                    out.push_str(&format!("{field}    purpose: {purpose}\n"));
                }
            }
        }
    }

    if let Some(list) = rows.children.get(&symbol.id) {
        for child in list {
            write_symbol(child, rows, depth + 1, out)?;
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
    use crate::test_support::{commit, init_repo};
    use regex::Regex;

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
        build_index(&store, repo.path(), None, &regex, None, None)
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

    #[tokio::test]
    async fn render_lists_commits_and_tickets_for_a_git_backed_symbol() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
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
        build_index(&store, repo.path(), None, &regex, None, None)
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

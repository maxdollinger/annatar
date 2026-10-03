//! Render a symbol and its descendants as text.
//!
//! [`render`] looks a symbol up by its fully qualified name and prints its
//! stored fields, then every child — nested types and members, recursively —
//! sorted by source line. This is the read side of the index: `annatar show`
//! and later the MCP `get_symbol` tool both build on it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use libsql::{Connection, Row};

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
    write_symbol(root, &children, 0, &mut out);
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

fn write_symbol(
    symbol: &StoredSymbol,
    children: &HashMap<i64, Vec<&StoredSymbol>>,
    depth: usize,
    out: &mut String,
) {
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

    let annotations = parse_annotations(&symbol.annotations);
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

    if let Some(list) = children.get(&symbol.id) {
        for child in list {
            write_symbol(child, children, depth + 1, out);
        }
    }
}

fn parse_annotations(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::build_index;
    use crate::store::Store;

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

    async fn indexed() -> (tempfile::TempDir, tempfile::TempDir, Store) {
        let repo = tempfile::tempdir().unwrap();
        let file = repo.path().join("src/main/java/com/acme/show/Widget.java");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, SOURCE).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        build_index(&store, repo.path(), None).await.unwrap();
        (repo, data, store)
    }

    #[tokio::test]
    async fn renders_a_symbol_and_its_children_in_source_order() {
        let (_repo, _data, store) = indexed().await;
        let conn = store.open_index().await.unwrap();

        let output = render(&conn, "com.acme.show.Widget").await.unwrap();

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
        let (_repo, _data, store) = indexed().await;
        let conn = store.open_index().await.unwrap();

        let err = render(&conn, "com.acme.show.Missing")
            .await
            .expect_err("unknown fqn should fail");
        assert!(
            format!("{err:#}").contains("com.acme.show.Missing"),
            "error should name the missing fqn, got: {err:#}"
        );
    }
}

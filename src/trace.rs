//! The transitive callers of a symbol as an indented tree (6.6).
//!
//! [`render`] walks [`usage_query::callers`] from the named symbol, one query
//! per symbol: its callers over `call`, `instantiate` and `reference` edges,
//! through the methods it overrides (`via`), one line per caller with the
//! lines of its mentions, then the caller's own callers indented below it,
//! depth first. A type is the root of its members' and nested types'
//! callers (without the ones inside it, as `show`'s `used by`); below the
//! root nothing is rolled up, and a type that only mentions the symbol above
//! it (a field, a header, an annotation) ends its branch (`[type, not
//! followed]`): its own users only hold it, the members that use the field
//! show up as callers of their own. A type whose initializers call the
//! symbol above (`[initializer]`) is followed through the code constructing
//! it ([`usage_query::constructions`]). Each symbol is expanded once, at its
//! shallowest occurrence (the first of them depth first, found by a
//! breadth-first pass); a later repeat prints `(see above)`, an earlier and
//! deeper one `(see below)`, so cycles and diamonds end and the depth never
//! hides a caller the tree reaches higher up. Entry points are marked
//! (`[entry: @Scheduled]`) and followed further when code calls them too; a
//! branch ends at a symbol without callers (`[no callers in main sources]`)
//! and at the depth limit (`[N callers beyond depth D]`). Each symbol lists
//! at most `limit` callers, the rest counted as `… N more`.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use libsql::{Connection, params};

use crate::usage_query::{self, Usage};

/// Caller levels below the root unless asked otherwise.
pub const DEFAULT_DEPTH: usize = 6;

/// The most caller levels a trace prints.
pub const MAX_DEPTH: usize = 10;

/// Callers listed per symbol unless asked otherwise.
pub const DEFAULT_LIMIT: usize = 10;

/// One way a caller reaches the symbol above it, with the lines of those
/// mentions in the caller's file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    kind: String,
    ambiguous: bool,
    via: Option<String>,
    lines: Vec<i64>,
}

/// A caller of the symbol above it in the tree, with all its links to it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Caller {
    id: i64,
    fqn: String,
    file: String,
    /// A class, interface, enum, record or annotation, not a member.
    is_type: bool,
    entry: Option<String>,
    links: Vec<Link>,
}

/// `usages` (ordered by file and line) as one [`Caller`] per symbol, in the
/// order of its first mention; its links merged by kind, `ambiguous` and
/// `via`, lines deduplicated.
fn group(usages: Vec<Usage>) -> Vec<Caller> {
    let mut callers: Vec<Caller> = Vec::new();
    for usage in usages {
        let index = match callers.iter().position(|caller| caller.id == usage.id) {
            Some(index) => index,
            None => {
                callers.push(Caller {
                    id: usage.id,
                    fqn: usage.fqn,
                    file: usage.file,
                    is_type: !matches!(usage.symbol_kind.as_str(), "method" | "constructor"),
                    entry: usage.entry,
                    links: Vec::new(),
                });
                callers.len() - 1
            }
        };
        let links = &mut callers[index].links;
        match links.iter_mut().find(|link| {
            link.kind == usage.kind && link.ambiguous == usage.ambiguous && link.via == usage.via
        }) {
            Some(link) => {
                if !link.lines.contains(&usage.line) {
                    link.lines.push(usage.line);
                }
            }
            None => links.push(Link {
                kind: usage.kind,
                ambiguous: usage.ambiguous,
                via: usage.via,
                lines: vec![usage.line],
            }),
        }
    }
    callers
}

impl Caller {
    /// Whether the trace goes on above it: a member, or a type whose
    /// initializers (a `call` or `instantiate` from the type itself, a field
    /// initializer or an initializer block) reach the symbol below, run by
    /// whatever constructs the type.
    fn followed(&self) -> bool {
        !self.is_type || self.links.iter().any(|link| link.kind != "reference")
    }
}

/// The callers of `caller` one level up: a member's callers (through the
/// methods it overrides), a type's constructions.
async fn callers_of(conn: &Connection, caller: &Caller) -> Result<Vec<Caller>> {
    let usages = if caller.is_type {
        usage_query::constructions(conn, caller.id).await?
    } else {
        usage_query::callers(conn, caller.id, false).await?
    };
    Ok(group(usages))
}

/// What is left to print, depth first.
enum Item {
    Caller {
        caller: Caller,
        depth: usize,
        /// The file of the symbol it calls, which a caller in the same file
        /// leaves out of its location.
        above: String,
    },
    More {
        hidden: usize,
        depth: usize,
    },
}

/// Render the callers of the symbol named `fqn` up to `depth` levels below
/// it, with at most `limit` callers per symbol.
///
/// The first line is the symbol, `fqn [kind] file:start-end`; below it each
/// caller as `fqn [kind] [ambiguous] [via I#m] file:line, line` (the kind left
/// out for `call`, the file left out when it is the file of the symbol it
/// calls, further links to the same symbol after `; `), indented two spaces
/// per level, then `[initializer]` and `[entry: @A]` where they apply,
/// ending in `(see above)`, `(see below)`, `[type, not followed]`, `[no
/// callers in main sources]` or `[N callers beyond depth D]` where the
/// branch stops. Each symbol is expanded at its shallowest occurrence, the
/// first of them depth first: a breadth-first pass finds the levels, the
/// depth-first one prints. A missing `fqn` is an error that names the
/// symbol.
pub async fn render(conn: &Connection, fqn: &str, depth: usize, limit: usize) -> Result<String> {
    let mut rows = conn
        .query(
            "SELECT id, kind, file, start_line, end_line, signature, annotations FROM symbols WHERE fqn = ?1",
            params![fqn],
        )
        .await
        .context("reading symbols")?;
    let row = rows
        .next()
        .await
        .context("reading symbols row")?
        .with_context(|| format!("symbol `{fqn}` not found in the index"))?;
    let id: i64 = row.get(0)?;
    let kind: String = row.get(1)?;
    let file: String = row.get(2)?;
    let start_line: i64 = row.get(3)?;
    let end_line: i64 = row.get(4)?;
    let signature: String = row.get(5)?;
    let annotations: String = row.get(6)?;
    let annotations: Vec<String> = serde_json::from_str(&annotations)
        .with_context(|| format!("decoding the annotations of `{fqn}`"))?;

    let mut out = format!("{fqn} [{kind}] {file}:{start_line}-{end_line}");
    if let Some(entry) = usage_query::entry_point(&kind, fqn, &signature, &annotations) {
        out.push_str(&format!(" [entry: {entry}]"));
    }
    let callers = group(usage_query::callers(conn, id, true).await?);
    if callers.is_empty() {
        out.push_str(" [no callers in main sources]");
    }
    out.push('\n');

    let mut lists = HashMap::from([(id, callers)]);
    let mut levels = HashMap::from([(id, 0)]);
    let mut queue = VecDeque::from([(id, 0)]);
    while let Some((next, level)) = queue.pop_front() {
        let found: Vec<Caller> = lists[&next]
            .iter()
            .take(limit)
            .filter(|caller| caller.followed() && !levels.contains_key(&caller.id))
            .cloned()
            .collect();
        for caller in found {
            levels.insert(caller.id, level + 1);
            lists.insert(caller.id, callers_of(conn, &caller).await?);
            if level + 1 < depth {
                queue.push_back((caller.id, level + 1));
            }
        }
    }

    let mut expanded = HashSet::from([id]);
    let mut stack = Vec::new();
    push(
        &mut stack,
        lists.remove(&id).unwrap_or_default(),
        1,
        &file,
        limit,
    );
    while let Some(item) = stack.pop() {
        let (caller, level, above) = match item {
            Item::More { hidden, depth } => {
                out.push_str(&format!("{}… {hidden} more\n", "  ".repeat(depth)));
                continue;
            }
            Item::Caller {
                caller,
                depth,
                above,
            } => (caller, depth, above),
        };
        out.push_str(&"  ".repeat(level));
        out.push_str(&caller_line(&caller, &above));
        if !caller.followed() {
            out.push_str(" [type, not followed]\n");
            continue;
        }
        if caller.is_type {
            out.push_str(" [initializer]");
        }
        if let Some(entry) = &caller.entry {
            out.push_str(&format!(" [entry: {entry}]"));
        }
        if expanded.contains(&caller.id) {
            out.push_str(" (see above)\n");
            continue;
        }
        if levels
            .get(&caller.id)
            .is_some_and(|&shallowest| shallowest < level)
        {
            out.push_str(" (see below)\n");
            continue;
        }
        expanded.insert(caller.id);
        let callers = lists.remove(&caller.id).unwrap_or_default();
        if callers.is_empty() {
            if caller.entry.is_none() {
                out.push_str(" [no callers in main sources]");
            }
            out.push('\n');
        } else if level == depth {
            let count = callers.len();
            let noun = if count == 1 { "caller" } else { "callers" };
            out.push_str(&format!(" [{count} {noun} beyond depth {depth}]\n"));
        } else {
            out.push('\n');
            push(&mut stack, callers, level + 1, &caller.file, limit);
        }
    }
    Ok(out)
}

/// Queue the first `limit` of `callers` at `depth` so they pop in order,
/// followed by `… N more` for the rest.
fn push(stack: &mut Vec<Item>, mut callers: Vec<Caller>, depth: usize, above: &str, limit: usize) {
    if callers.len() > limit {
        stack.push(Item::More {
            hidden: callers.len() - limit,
            depth,
        });
        callers.truncate(limit);
    }
    for caller in callers.into_iter().rev() {
        stack.push(Item::Caller {
            caller,
            depth,
            above: above.to_string(),
        });
    }
}

/// `fqn` and the caller's links, `[kind] [ambiguous] [via I#m] file:lines`
/// joined by `; `, the file only on the first and only when it is not
/// `above`.
fn caller_line(caller: &Caller, above: &str) -> String {
    let links: Vec<String> = caller
        .links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            let mut text = String::new();
            if link.kind != "call" {
                text.push_str(&link.kind);
                text.push(' ');
            }
            if link.ambiguous {
                text.push_str("ambiguous ");
            }
            if let Some(via) = &link.via {
                text.push_str(&format!("via {via} "));
            }
            if index == 0 && caller.file != above {
                text.push_str(&caller.file);
            }
            let lines: Vec<String> = link.lines.iter().map(ToString::to_string).collect();
            text.push_str(&format!(":{}", lines.join(", ")));
            text
        })
        .collect();
    format!("{} {}", caller.fqn, links.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_TICKET_REGEX;
    use crate::indexer::build_index;
    use crate::store::{IndexReader, Store};
    use crate::tickets::JiraMode;
    use regex::Regex;

    const FLOW: [(&str, &str); 6] = [
        (
            "Message.java",
            "\
package com.acme.trace;

public class Message {
}
",
        ),
        (
            "Consumer.java",
            "\
package com.acme.trace;

public interface Consumer {
    void consume(Message message);
}
",
        ),
        (
            "AddConsumer.java",
            "\
package com.acme.trace;

public class AddConsumer implements Consumer {
    @Override
    public void consume(Message message) {
        audit(message);
    }

    private void audit(Message message) {
    }
}
",
        ),
        (
            "Dispatcher.java",
            "\
package com.acme.trace;

public class Dispatcher {
    private final Consumer consumer;
    private final AddConsumer addConsumer;

    public Dispatcher(Consumer consumer, AddConsumer addConsumer) {
        this.consumer = consumer;
        this.addConsumer = addConsumer;
    }

    @Scheduled(fixedDelay = 1)
    public void process() {
        left(new Message());
        right(new Message());
    }

    void left(Message message) {
        dispatch(message);
    }

    void right(Message message) {
        dispatch(message);
    }

    void dispatch(Message message) {
        consumer.consume(message);
        addConsumer.consume(message);
    }
}
",
        ),
        (
            "Retry.java",
            "\
package com.acme.trace;

public class Retry {
    private final AddConsumer addConsumer = new AddConsumer();

    void first(Message message) {
        addConsumer.consume(message);
        second(message);
    }

    void second(Message message) {
        first(message);
    }
}
",
        ),
        (
            "Manual.java",
            "\
package com.acme.trace;

public class Manual {
    void run() {
        new Retry().first(new Message());
    }
}
",
        ),
    ];

    async fn flow() -> (tempfile::TempDir, IndexReader) {
        index(&FLOW).await
    }

    async fn index(files: &[(&str, &str)]) -> (tempfile::TempDir, IndexReader) {
        let repo = tempfile::tempdir().unwrap();
        for (name, source) in files {
            let package = source
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("package "))
                .and_then(|line| line.strip_suffix(';'))
                .unwrap();
            let dir = repo
                .path()
                .join("src/main/java")
                .join(package.replace('.', "/"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), source).unwrap();
        }
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let regex = Regex::new(DEFAULT_TICKET_REGEX).unwrap();
        build_index(&store, repo.path(), None, &regex, &JiraMode::Disabled, None)
            .await
            .unwrap();
        drop(store);
        let reader = IndexReader::open(data.path()).await.unwrap();
        (data, reader)
    }

    async fn trace(reader: &IndexReader, fqn: &str, depth: usize, limit: usize) -> String {
        render(reader.connection(), fqn, depth, limit)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_method_traces_its_callers_through_via_a_diamond_and_a_cycle_to_their_leaves() {
        let (_data, reader) = flow().await;

        let output = trace(
            &reader,
            "com.acme.trace.AddConsumer#consume(Message)",
            4,
            10,
        )
        .await;

        assert_eq!(
            output,
            "\
com.acme.trace.AddConsumer#consume(Message) [method] src/main/java/com/acme/trace/AddConsumer.java:4-7
  com.acme.trace.Dispatcher#dispatch(Message) via com.acme.trace.Consumer#consume(Message) src/main/java/com/acme/trace/Dispatcher.java:27; :28
    com.acme.trace.Dispatcher#left(Message) :19
      com.acme.trace.Dispatcher#process() :14 [entry: @Scheduled]
    com.acme.trace.Dispatcher#right(Message) :23
      com.acme.trace.Dispatcher#process() :15 [entry: @Scheduled] (see above)
  com.acme.trace.Retry#first(Message) src/main/java/com/acme/trace/Retry.java:7
    com.acme.trace.Manual#run() src/main/java/com/acme/trace/Manual.java:5 [no callers in main sources]
    com.acme.trace.Retry#second(Message) :12
      com.acme.trace.Retry#first(Message) :8 (see above)
"
        );
    }

    #[tokio::test]
    async fn the_depth_ends_a_branch_with_its_callers_counted() {
        let (_data, reader) = flow().await;

        let output = trace(
            &reader,
            "com.acme.trace.AddConsumer#consume(Message)",
            2,
            10,
        )
        .await;

        assert_eq!(
            output,
            "\
com.acme.trace.AddConsumer#consume(Message) [method] src/main/java/com/acme/trace/AddConsumer.java:4-7
  com.acme.trace.Dispatcher#dispatch(Message) via com.acme.trace.Consumer#consume(Message) src/main/java/com/acme/trace/Dispatcher.java:27; :28
    com.acme.trace.Dispatcher#left(Message) :19 [1 caller beyond depth 2]
    com.acme.trace.Dispatcher#right(Message) :23 [1 caller beyond depth 2]
  com.acme.trace.Retry#first(Message) src/main/java/com/acme/trace/Retry.java:7
    com.acme.trace.Manual#run() src/main/java/com/acme/trace/Manual.java:5 [no callers in main sources]
    com.acme.trace.Retry#second(Message) :12 [1 caller beyond depth 2]
"
        );
    }

    #[tokio::test]
    async fn a_symbol_is_expanded_where_it_is_shallowest_and_a_deeper_copy_before_points_below() {
        let (_data, reader) = flow().await;

        let output = trace(&reader, "com.acme.trace.Message", 2, 10).await;

        assert_eq!(
            output,
            "\
com.acme.trace.Message [class] src/main/java/com/acme/trace/Message.java:3-4
  com.acme.trace.AddConsumer#consume(Message) reference src/main/java/com/acme/trace/AddConsumer.java:5
    com.acme.trace.Dispatcher#dispatch(Message) via com.acme.trace.Consumer#consume(Message) src/main/java/com/acme/trace/Dispatcher.java:27; :28 (see below)
    com.acme.trace.Retry#first(Message) src/main/java/com/acme/trace/Retry.java:7 (see below)
  com.acme.trace.AddConsumer#audit(Message) reference src/main/java/com/acme/trace/AddConsumer.java:9
    com.acme.trace.AddConsumer#consume(Message) :6 (see above)
  com.acme.trace.Consumer#consume(Message) reference src/main/java/com/acme/trace/Consumer.java:4
    com.acme.trace.Dispatcher#dispatch(Message) src/main/java/com/acme/trace/Dispatcher.java:27 (see below)
  com.acme.trace.Dispatcher#process() instantiate src/main/java/com/acme/trace/Dispatcher.java:14, 15 [entry: @Scheduled]
  com.acme.trace.Dispatcher#left(Message) reference src/main/java/com/acme/trace/Dispatcher.java:18
    com.acme.trace.Dispatcher#process() :14 [entry: @Scheduled] (see above)
  com.acme.trace.Dispatcher#right(Message) reference src/main/java/com/acme/trace/Dispatcher.java:22
    com.acme.trace.Dispatcher#process() :15 [entry: @Scheduled] (see above)
  com.acme.trace.Dispatcher#dispatch(Message) reference src/main/java/com/acme/trace/Dispatcher.java:26
    com.acme.trace.Dispatcher#left(Message) :19 (see above)
    com.acme.trace.Dispatcher#right(Message) :23 (see above)
  com.acme.trace.Manual#run() instantiate src/main/java/com/acme/trace/Manual.java:5 [no callers in main sources]
  com.acme.trace.Retry#first(Message) reference src/main/java/com/acme/trace/Retry.java:6
    com.acme.trace.Manual#run() src/main/java/com/acme/trace/Manual.java:5 (see above)
    com.acme.trace.Retry#second(Message) :12 (see below)
  com.acme.trace.Retry#second(Message) reference src/main/java/com/acme/trace/Retry.java:11
    com.acme.trace.Retry#first(Message) :8 (see above)
"
        );
    }

    const CHAIN: [(&str, &str); 7] = [
        (
            "R.java",
            "\
package com.acme.chain;

public class R {
    static void r() {
    }
}
",
        ),
        (
            "A.java",
            "\
package com.acme.chain;

public class A {
    static void a() {
        R.r();
    }
}
",
        ),
        (
            "B.java",
            "\
package com.acme.chain;

public class B {
    static void b() {
        A.a();
    }
}
",
        ),
        (
            "X.java",
            "\
package com.acme.chain;

public class X {
    static void x() {
        B.b();
        R.r();
    }
}
",
        ),
        (
            "Y.java",
            "\
package com.acme.chain;

public class Y {
    static void y() {
        X.x();
    }
}
",
        ),
        (
            "Z.java",
            "\
package com.acme.chain;

public class Z {
    @Bean
    static Object z() {
        Y.y();
        return null;
    }
}
",
        ),
        (
            "W.java",
            "\
package com.acme.chain;

public class W {
    @Scheduled(fixedDelay = 1)
    void w() {
        Z.z();
    }
}
",
        ),
    ];

    #[tokio::test]
    async fn a_symbol_reached_deep_first_is_expanded_higher_up_past_an_entry_point_with_callers() {
        let (_data, reader) = index(&CHAIN).await;

        let output = trace(&reader, "com.acme.chain.R#r()", 4, 10).await;

        assert_eq!(
            output,
            "\
com.acme.chain.R#r() [method] src/main/java/com/acme/chain/R.java:4-5
  com.acme.chain.A#a() src/main/java/com/acme/chain/A.java:5
    com.acme.chain.B#b() src/main/java/com/acme/chain/B.java:5
      com.acme.chain.X#x() src/main/java/com/acme/chain/X.java:5 (see below)
  com.acme.chain.X#x() src/main/java/com/acme/chain/X.java:6
    com.acme.chain.Y#y() src/main/java/com/acme/chain/Y.java:5
      com.acme.chain.Z#z() src/main/java/com/acme/chain/Z.java:6 [entry: @Bean]
        com.acme.chain.W#w() src/main/java/com/acme/chain/W.java:6 [entry: @Scheduled]
"
        );
    }

    #[tokio::test]
    async fn each_symbol_lists_at_most_limit_callers() {
        let (_data, reader) = flow().await;

        let output = trace(&reader, "com.acme.trace.AddConsumer#consume(Message)", 4, 1).await;

        assert_eq!(
            output,
            "\
com.acme.trace.AddConsumer#consume(Message) [method] src/main/java/com/acme/trace/AddConsumer.java:4-7
  com.acme.trace.Dispatcher#dispatch(Message) via com.acme.trace.Consumer#consume(Message) src/main/java/com/acme/trace/Dispatcher.java:27; :28
    com.acme.trace.Dispatcher#left(Message) :19
      com.acme.trace.Dispatcher#process() :14 [entry: @Scheduled]
    … 1 more
  … 1 more
"
        );
    }

    #[tokio::test]
    async fn a_type_traces_its_members_callers_stops_at_types_and_follows_initializers() {
        let (_data, reader) = flow().await;

        let output = trace(&reader, "com.acme.trace.AddConsumer", 4, 10).await;

        assert_eq!(
            output,
            "\
com.acme.trace.AddConsumer [class] src/main/java/com/acme/trace/AddConsumer.java:3-11
  com.acme.trace.Dispatcher reference src/main/java/com/acme/trace/Dispatcher.java:5 [type, not followed]
  com.acme.trace.Dispatcher#<init>(Consumer,AddConsumer) reference src/main/java/com/acme/trace/Dispatcher.java:7 [no callers in main sources]
  com.acme.trace.Dispatcher#dispatch(Message) via com.acme.trace.Consumer#consume(Message) src/main/java/com/acme/trace/Dispatcher.java:27; :28
    com.acme.trace.Dispatcher#left(Message) :19
      com.acme.trace.Dispatcher#process() :14 [entry: @Scheduled]
    com.acme.trace.Dispatcher#right(Message) :23
      com.acme.trace.Dispatcher#process() :15 [entry: @Scheduled] (see above)
  com.acme.trace.Retry instantiate src/main/java/com/acme/trace/Retry.java:4; reference :4 [initializer]
    com.acme.trace.Manual#run() instantiate src/main/java/com/acme/trace/Manual.java:5 [no callers in main sources]
  com.acme.trace.Retry#first(Message) src/main/java/com/acme/trace/Retry.java:7
    com.acme.trace.Manual#run() src/main/java/com/acme/trace/Manual.java:5 (see above)
    com.acme.trace.Retry#second(Message) :12
      com.acme.trace.Retry#first(Message) :8 (see above)
"
        );
    }

    #[tokio::test]
    async fn an_entry_point_without_callers_is_the_whole_trace() {
        let (_data, reader) = flow().await;

        let output = trace(&reader, "com.acme.trace.Dispatcher#process()", 4, 10).await;

        assert_eq!(
            output,
            "com.acme.trace.Dispatcher#process() [method] src/main/java/com/acme/trace/Dispatcher.java:12-16 [entry: @Scheduled] [no callers in main sources]\n"
        );
    }

    #[tokio::test]
    async fn an_unknown_fqn_is_an_error_that_names_it() {
        let (_data, reader) = flow().await;

        let err = render(reader.connection(), "com.acme.trace.Missing", 4, 10)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "symbol `com.acme.trace.Missing` not found in the index"
        );
    }

    #[test]
    fn a_caller_line_names_kind_ambiguous_and_via_and_the_file_only_when_it_changes() {
        let usage = |kind: &str, line, ambiguous, via: Option<&str>| Usage {
            id: 1,
            fqn: "a.A#f()".to_string(),
            file: "src/A.java".to_string(),
            symbol_kind: "method".to_string(),
            kind: kind.to_string(),
            line,
            ambiguous,
            via: via.map(str::to_string),
            entry: None,
        };
        let callers = group(vec![
            usage("call", 3, true, None),
            usage("call", 3, true, None),
            usage("call", 7, true, None),
            usage("call", 8, false, Some("a.I#m()")),
            usage("instantiate", 9, false, None),
        ]);

        assert_eq!(callers.len(), 1);
        assert_eq!(
            caller_line(&callers[0], "src/B.java"),
            "a.A#f() ambiguous src/A.java:3, 7; via a.I#m() :8; instantiate :9"
        );
        assert_eq!(
            caller_line(&callers[0], "src/A.java"),
            "a.A#f() ambiguous :3, 7; via a.I#m() :8; instantiate :9"
        );
    }
}

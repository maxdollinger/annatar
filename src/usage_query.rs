//! The direct usages of a symbol, read from the `edges` table (6.5).
//!
//! [`used_by`] is every edge into the symbol, for a type also into its
//! members and nested types, without the edges that start inside it
//! (`usages.md`); a method that overrides others (`overrides` edges, up the
//! chain) adds the callers of each method it overrides, labelled with that
//! method (`via`). [`uses`] is every edge out of the symbol, for a type also
//! out of its members and nested types, without the edges that end inside
//! it. In a type's roll-up the `overrides` edges of its members are left
//! out: the `extends` / `implements` of the type says the same. Both are
//! plain SQL joins over `parent_id` and `overrides`, read by `show`.
//! [`callers`] is the variant `trace` walks: only `call`, `instantiate` and
//! `reference` edges, a type rolled up only when asked; [`constructions`]
//! the code constructing a type. [`entry_point`]
//! names the annotations the framework calls a method through.

use anyhow::{Context, Result};
use libsql::{Connection, params};

/// One edge seen from the shown symbol: the other symbol (the user for
/// [`used_by`], the used one for [`uses`]) and the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub id: i64,
    pub fqn: String,
    pub file: String,
    /// The other symbol's kind (`class`, `method`, ..).
    pub symbol_kind: String,
    /// The edge kind.
    pub kind: String,
    /// The line of the mention, in the user's file.
    pub line: i64,
    pub ambiguous: bool,
    /// The overridden method the user calls, when the edge reaches the
    /// shown symbol only through an override.
    pub via: Option<String>,
    /// The other symbol's entry-point annotations, see [`entry_point`].
    pub entry: Option<String>,
}

/// The rows of the symbol `id` and everything nested in it.
const INSIDE: &str = "inside(id) AS (
    SELECT ?1
    UNION SELECT symbols.id FROM symbols JOIN inside ON symbols.parent_id = inside.id
)";

/// The row of the symbol `id` alone, for the queries without a roll-up.
const SELF: &str = "inside(id) AS (SELECT ?1)";

/// The row of the type `id` and its constructors.
const CONSTRUCTORS: &str = "inside(id) AS (
    SELECT ?1
    UNION SELECT id FROM symbols WHERE parent_id = ?1 AND kind = 'constructor'
)";

/// The edge kinds `trace` follows to a symbol's callers.
const CALLER_KINDS: &str = "AND edges.kind IN ('call', 'instantiate', 'reference')";

/// The edge kinds that construct a type.
const CONSTRUCTION_KINDS: &str = "AND edges.kind IN ('call', 'instantiate')";

/// The users of the symbol `id`: incoming edges of it and of its members
/// and nested types whose source is outside it (`overrides` only into the
/// symbol itself), then the non-`overrides` edges into every method they
/// override (transitively, outside it), with `via` set; ordered by file,
/// line and fqn.
pub async fn used_by(conn: &Connection, id: i64) -> Result<Vec<Usage>> {
    users(conn, id, INSIDE, "").await
}

/// The callers of the symbol `id` as `trace` follows them: the `call`,
/// `instantiate` and `reference` edges of [`used_by`] (with `via`), for a
/// type rolled up over its members and nested types only when `roll_up` is
/// set, otherwise into the symbol itself.
pub async fn callers(conn: &Connection, id: i64, roll_up: bool) -> Result<Vec<Usage>> {
    users(conn, id, if roll_up { INSIDE } else { SELF }, CALLER_KINDS).await
}

/// The code constructing the type `id`: the `instantiate` edges into it
/// and the `call` / `instantiate` edges into its constructors from outside
/// them, for `trace` to follow a type whose initializers call the symbol
/// above.
pub async fn constructions(conn: &Connection, id: i64) -> Result<Vec<Usage>> {
    users(conn, id, CONSTRUCTORS, CONSTRUCTION_KINDS).await
}

async fn users(conn: &Connection, id: i64, inside: &str, kinds: &str) -> Result<Vec<Usage>> {
    let sql = format!(
        "WITH RECURSIVE {inside},
         overridden(id) AS (
             SELECT dst_id FROM edges WHERE kind = 'overrides' AND src_id IN inside
             UNION SELECT edges.dst_id FROM edges JOIN overridden ON edges.src_id = overridden.id
             WHERE edges.kind = 'overrides'
         ),
         targets(id, via) AS (
             SELECT id, NULL FROM inside
             UNION SELECT id, id FROM overridden WHERE id NOT IN inside
         )
         SELECT src.id, src.fqn, src.file, src.kind, src.signature, src.annotations,
                edges.kind, edges.line, edges.ambiguous, via.fqn
         FROM targets
         JOIN edges ON edges.dst_id = targets.id
         JOIN symbols AS src ON src.id = edges.src_id
         JOIN symbols AS dst ON dst.id = edges.dst_id
         LEFT JOIN symbols AS via ON via.id = targets.via
         WHERE edges.src_id NOT IN inside
           AND (edges.kind <> 'overrides' OR (targets.via IS NULL AND edges.dst_id = ?1))
           {kinds}
         ORDER BY src.file, edges.line, src.fqn, edges.kind, via.fqn, edges.ambiguous, dst.fqn"
    );
    read(conn, &sql, id)
        .await
        .with_context(|| format!("reading the users of symbol {id}"))
}

/// The symbols the symbol `id` uses: outgoing edges of it and of its
/// members and nested types whose target is outside it (`overrides` only
/// out of the symbol itself, and no `instantiate` of a type whose
/// constructor the same line instantiates); ordered by the target's file,
/// the line and its fqn.
pub async fn uses(conn: &Connection, id: i64) -> Result<Vec<Usage>> {
    let sql = format!(
        "WITH RECURSIVE {INSIDE}
         SELECT dst.id, dst.fqn, dst.file, dst.kind, dst.signature, dst.annotations,
                edges.kind, edges.line, edges.ambiguous, NULL
         FROM edges
         JOIN symbols AS dst ON dst.id = edges.dst_id
         JOIN symbols AS src ON src.id = edges.src_id
         WHERE edges.src_id IN inside AND edges.dst_id NOT IN inside
           AND (edges.kind <> 'overrides' OR edges.src_id = ?1)
           AND NOT (edges.kind = 'instantiate' AND EXISTS (
               SELECT 1 FROM edges AS ctor
               JOIN symbols AS init ON init.id = ctor.dst_id
               WHERE ctor.src_id = edges.src_id AND ctor.line = edges.line
                 AND ctor.kind = 'instantiate' AND init.parent_id = edges.dst_id
           ))
         ORDER BY dst.file, edges.line, dst.fqn, edges.kind, edges.ambiguous, src.fqn"
    );
    read(conn, &sql, id)
        .await
        .with_context(|| format!("reading the uses of symbol {id}"))
}

async fn read(conn: &Connection, sql: &str, id: i64) -> Result<Vec<Usage>> {
    let mut rows = conn.query(sql, params![id]).await?;
    let mut usages = Vec::new();
    while let Some(row) = rows.next().await? {
        let fqn: String = row.get(1)?;
        let kind: String = row.get(3)?;
        let signature: String = row.get(4)?;
        let annotations: String = row.get(5)?;
        let annotations: Vec<String> = serde_json::from_str(&annotations)
            .with_context(|| format!("decoding the annotations of `{fqn}`"))?;
        let ambiguous: i64 = row.get(8)?;
        usages.push(Usage {
            id: row.get(0)?,
            entry: entry_point(&kind, &fqn, &signature, &annotations),
            fqn,
            file: row.get(2)?,
            symbol_kind: kind,
            kind: row.get(6)?,
            line: row.get(7)?,
            ambiguous: ambiguous != 0,
            via: row.get(9)?,
        });
    }
    Ok(usages)
}

/// The annotations that make a method an entry point: the framework calls
/// it, so it has no caller in the code.
pub const ENTRY_ANNOTATIONS: [&str; 12] = [
    "Scheduled",
    "RequestMapping",
    "GetMapping",
    "PostMapping",
    "PutMapping",
    "DeleteMapping",
    "PatchMapping",
    "EventListener",
    "ExceptionHandler",
    "PostConstruct",
    "PreDestroy",
    "Bean",
];

/// Why a symbol is an entry point, when it is a method: its entry-point
/// annotations by simple name (`@Scheduled`, `@GetMapping`, several joined
/// by `, `), or `public static void main` for a `main(String[])` that is
/// public and static; `None` for anything else.
pub fn entry_point(
    kind: &str,
    fqn: &str,
    signature: &str,
    annotations: &[String],
) -> Option<String> {
    if kind != "method" {
        return None;
    }
    let mut names: Vec<String> = annotations
        .iter()
        .filter_map(|annotation| {
            let name = annotation.strip_prefix('@')?;
            let name = name.split('(').next()?.trim();
            let simple = name.rsplit('.').next()?;
            ENTRY_ANNOTATIONS
                .contains(&simple)
                .then(|| format!("@{simple}"))
        })
        .collect();
    names.dedup();
    if names.is_empty() && is_main(fqn, signature) {
        names.push("public static void main".to_string());
    }
    (!names.is_empty()).then(|| names.join(", "))
}

fn is_main(fqn: &str, signature: &str) -> bool {
    let Some((_, member)) = fqn.rsplit_once('#') else {
        return false;
    };
    let words: Vec<&str> = signature
        .split(|c: char| c.is_whitespace() || c == '(')
        .collect();
    matches!(
        member,
        "main(String[])" | "main(String...)" | "main(java.lang.String[])"
    ) && ["public", "static", "void"]
        .iter()
        .all(|word| words.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{IndexReader, Store};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn entry_points_by_annotation_and_main() {
        let entry = |kind, fqn, signature, annotations: &[&str]| {
            entry_point(kind, fqn, signature, &strings(annotations))
        };
        assert_eq!(
            entry(
                "method",
                "a.D#run()",
                "void run()",
                &["@Scheduled(fixedDelay = 1)"]
            ),
            Some("@Scheduled".to_string())
        );
        assert_eq!(
            entry(
                "method",
                "a.D#get()",
                "User get()",
                &[
                    "@Override",
                    "@org.springframework.web.bind.annotation.GetMapping(\"/{id}\")",
                    "@Bean"
                ]
            ),
            Some("@GetMapping, @Bean".to_string())
        );
        assert_eq!(
            entry(
                "method",
                "a.Advice#handle(IllegalStateException)",
                "ResponseEntity<String> handle(IllegalStateException e)",
                &["@ExceptionHandler(IllegalStateException.class)"]
            ),
            Some("@ExceptionHandler".to_string())
        );
        assert_eq!(
            entry("method", "a.Pool#close()", "void close()", &["@PreDestroy"]),
            Some("@PreDestroy".to_string())
        );
        assert_eq!(
            entry(
                "method",
                "a.App#main(String[])",
                "public static void main(String[] args)",
                &[]
            ),
            Some("public static void main".to_string())
        );
        assert_eq!(
            entry(
                "method",
                "a.App#main(String[])",
                "void main(String[] args)",
                &[]
            ),
            None
        );
        assert_eq!(
            entry(
                "method",
                "a.App#main(String)",
                "public static void main(String a)",
                &[]
            ),
            None
        );
        assert_eq!(
            entry("class", "a.Web", "class Web", &["@RequestMapping(\"/x\")"]),
            None
        );
        assert_eq!(
            entry(
                "method",
                "a.D#m()",
                "void m()",
                &["@Transactional", "@Scheduler"]
            ),
            None
        );
    }

    /// An index with `symbols` (fqn, kind, parent fqn, annotations) and
    /// `edges` (src, dst, kind, line; a kind ending in `?` is ambiguous).
    async fn index_with(
        symbols: &[(&str, &str, Option<&str>, &str)],
        edges: &[(&str, &str, &str, i64)],
    ) -> (tempfile::TempDir, IndexReader) {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        let conn = build.connection();
        for (fqn, kind, parent, annotations) in symbols {
            let file = format!("{}.java", fqn.split('#').next().unwrap());
            conn.execute(
                "INSERT INTO symbols (parent_id, kind, fqn, file, start_line, end_line, signature, annotations, content_hash) \
                 VALUES ((SELECT id FROM symbols WHERE fqn = ?3), ?1, ?2, ?4, 1, 1, '', ?5, '')",
                params![*kind, *fqn, *parent, file, *annotations],
            )
            .await
            .unwrap();
        }
        for (src, dst, kind, line) in edges {
            let ambiguous = kind.ends_with('?');
            conn.execute(
                "INSERT INTO edges (src_id, dst_id, kind, line, ambiguous) VALUES \
                 ((SELECT id FROM symbols WHERE fqn = ?1), (SELECT id FROM symbols WHERE fqn = ?2), ?3, ?4, ?5)",
                params![*src, *dst, kind.trim_end_matches('?'), *line, ambiguous],
            )
            .await
            .unwrap();
        }
        build.commit().unwrap();
        drop(store);
        let reader = IndexReader::open(data.path()).await.unwrap();
        (data, reader)
    }

    async fn id(conn: &Connection, fqn: &str) -> i64 {
        let mut rows = conn
            .query("SELECT id FROM symbols WHERE fqn = ?1", [fqn])
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    fn lines(usages: &[Usage]) -> Vec<String> {
        usages
            .iter()
            .map(|usage| {
                format!(
                    "{} {}{} :{}{}{}",
                    usage.fqn,
                    usage.kind,
                    if usage.ambiguous { " ambiguous" } else { "" },
                    usage.line,
                    usage
                        .via
                        .as_ref()
                        .map(|via| format!(" via {via}"))
                        .unwrap_or_default(),
                    usage
                        .entry
                        .as_ref()
                        .map(|entry| format!(" [{entry}]"))
                        .unwrap_or_default()
                )
            })
            .collect()
    }

    async fn sample() -> (tempfile::TempDir, IndexReader) {
        index_with(
            &[
                ("a.I", "interface", None, "[]"),
                ("a.I#m()", "method", Some("a.I"), "[]"),
                ("a.B", "class", None, "[]"),
                ("a.B#m()", "method", Some("a.B"), "[]"),
                ("a.C", "class", None, "[]"),
                ("a.C#m()", "method", Some("a.C"), "[]"),
                ("a.C#help()", "method", Some("a.C"), "[]"),
                ("a.C.Inner", "class", Some("a.C"), "[]"),
                ("a.C.Inner#go()", "method", Some("a.C.Inner"), "[]"),
                ("a.D", "class", None, "[]"),
                (
                    "a.D#run()",
                    "method",
                    Some("a.D"),
                    "[\"@Scheduled(fixedDelay = 1)\"]",
                ),
                ("a.D#direct()", "method", Some("a.D"), "[]"),
                ("a.J", "interface", None, "[]"),
                ("a.J#m()", "method", Some("a.J"), "[]"),
                ("a.E", "class", None, "[]"),
                ("a.E#viaB()", "method", Some("a.E"), "[]"),
                ("a.E#viaJ()", "method", Some("a.E"), "[]"),
                ("a.E#pick()", "method", Some("a.E"), "[]"),
                ("a.Lib", "class", None, "[]"),
                ("a.Lib#<init>()", "constructor", Some("a.Lib"), "[]"),
            ],
            &[
                ("a.B", "a.I", "implements", 3),
                ("a.B#m()", "a.I#m()", "overrides", 4),
                ("a.J#m()", "a.I#m()", "overrides", 4),
                ("a.C", "a.B", "extends", 3),
                ("a.C", "a.J", "implements", 3),
                ("a.C#m()", "a.B#m()", "overrides", 5),
                ("a.C#m()", "a.J#m()", "overrides", 5),
                ("a.C#m()", "a.Lib", "instantiate", 11),
                ("a.C#m()", "a.Lib#<init>()", "instantiate", 11),
                ("a.C#m()", "a.C#help()", "call", 6),
                ("a.C#m()", "a.Lib", "reference", 7),
                ("a.C.Inner#go()", "a.C#help()", "call", 12),
                ("a.C.Inner#go()", "a.Lib", "reference", 13),
                ("a.D#run()", "a.I#m()", "call", 8),
                ("a.D#direct()", "a.C#m()", "call", 9),
                ("a.D#direct()", "a.C.Inner#go()", "call", 10),
                ("a.D", "a.C", "reference", 2),
                ("a.E#viaB()", "a.B#m()", "call", 20),
                ("a.E#viaJ()", "a.J#m()", "call", 21),
                ("a.E#pick()", "a.C#m()", "call?", 22),
                ("a.E#pick()", "a.C#help()", "call?", 22),
                ("a.E#pick()", "a.C#help()", "call", 23),
            ],
        )
        .await
    }

    #[tokio::test]
    async fn a_method_is_used_by_its_callers_and_the_callers_of_each_method_it_overrides_once() {
        let (_data, reader) = sample().await;
        let conn = reader.connection();

        let users = used_by(conn, id(conn, "a.C#m()").await).await.unwrap();

        assert_eq!(
            lines(&users),
            strings(&[
                "a.D#run() call :8 via a.I#m() [@Scheduled]",
                "a.D#direct() call :9",
                "a.E#viaB() call :20 via a.B#m()",
                "a.E#viaJ() call :21 via a.J#m()",
                "a.E#pick() call ambiguous :22",
            ])
        );
        let users = used_by(conn, id(conn, "a.B#m()").await).await.unwrap();
        assert_eq!(
            lines(&users),
            strings(&[
                "a.C#m() overrides :5",
                "a.D#run() call :8 via a.I#m() [@Scheduled]",
                "a.E#viaB() call :20",
            ])
        );
        let users = used_by(conn, id(conn, "a.I#m()").await).await.unwrap();
        assert_eq!(
            lines(&users),
            strings(&[
                "a.B#m() overrides :4",
                "a.D#run() call :8 [@Scheduled]",
                "a.J#m() overrides :4",
            ])
        );
        let uses = uses(conn, id(conn, "a.C#m()").await).await.unwrap();
        assert_eq!(
            lines(&uses),
            strings(&[
                "a.B#m() overrides :5",
                "a.C#help() call :6",
                "a.J#m() overrides :5",
                "a.Lib reference :7",
                "a.Lib#<init>() instantiate :11",
            ])
        );
    }

    #[tokio::test]
    async fn a_type_rolls_up_its_members_and_nested_types_without_its_own_edges() {
        let (_data, reader) = sample().await;
        let conn = reader.connection();

        let users = used_by(conn, id(conn, "a.C").await).await.unwrap();

        assert_eq!(
            lines(&users),
            strings(&[
                "a.D reference :2",
                "a.D#run() call :8 via a.I#m() [@Scheduled]",
                "a.D#direct() call :9",
                "a.D#direct() call :10",
                "a.E#viaB() call :20 via a.B#m()",
                "a.E#viaJ() call :21 via a.J#m()",
                "a.E#pick() call ambiguous :22",
                "a.E#pick() call ambiguous :22",
                "a.E#pick() call :23",
            ])
        );
        let uses = uses(conn, id(conn, "a.C").await).await.unwrap();
        assert_eq!(
            lines(&uses),
            strings(&[
                "a.B extends :3",
                "a.J implements :3",
                "a.Lib reference :7",
                "a.Lib#<init>() instantiate :11",
                "a.Lib reference :13",
            ])
        );
    }

    #[tokio::test]
    async fn callers_follow_only_usage_kinds_and_roll_a_type_up_only_when_asked() {
        let (_data, reader) = sample().await;
        let conn = reader.connection();

        let callers_of = |fqn: &'static str, roll_up| async move {
            lines(&callers(conn, id(conn, fqn).await, roll_up).await.unwrap())
        };

        assert_eq!(
            callers_of("a.B#m()", false).await,
            strings(&[
                "a.D#run() call :8 via a.I#m() [@Scheduled]",
                "a.E#viaB() call :20",
            ])
        );
        assert_eq!(
            callers_of("a.C", false).await,
            strings(&["a.D reference :2"])
        );
        assert!(callers_of("a.B", false).await.is_empty());
        assert_eq!(
            callers_of("a.B", true).await,
            strings(&[
                "a.D#run() call :8 via a.I#m() [@Scheduled]",
                "a.E#viaB() call :20",
            ])
        );
        assert_eq!(
            callers_of("a.C", true).await,
            lines(&used_by(conn, id(conn, "a.C").await).await.unwrap())
        );
    }

    #[tokio::test]
    async fn a_symbol_without_users_or_uses_has_none() {
        let (_data, reader) = sample().await;
        let conn = reader.connection();
        let run = id(conn, "a.D").await;

        assert!(used_by(conn, run).await.unwrap().is_empty());
        assert!(
            uses(conn, id(conn, "a.Lib").await)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

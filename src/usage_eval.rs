//! Usage quality against a usage golden set: `annatar eval-usages` (6.4).
//!
//! A usage golden set is a TOML file of `[[symbol]]` tables, each a symbol
//! with its true direct users, found by hand (or an IDE's *Find usages*)
//! in the main sources:
//!
//! ```toml
//! [[symbol]]
//! fqn = "com.acme.Consumer#consume(AddMessage)"
//! kind = "method"
//! users = ["com.acme.Dispatcher#dispatch(Message)"]
//! overridden_by = ["com.acme.LoggingConsumer#consume(AddMessage)"]
//! note = "reached only through a getter chain"
//! ```
//!
//! A *direct user* is the symbol an edge starts at: the innermost method or
//! constructor around a mention (lambdas and anonymous classes belong to
//! it), else the type (fields, header, annotations). A type's users roll
//! up the users of its members and nested types, without the ones inside
//! the type itself (`usages.md`). Every edge kind but `overrides` counts;
//! the overriding methods are `overridden_by`, scored apart and only when
//! the key is present. `kind` (optional) is the symbol's kind, `note` free
//! text. [`UsageSet::parse`] validates the file; [`evaluate`] checks every
//! fqn against the index first (a renamed symbol fails loudly, as
//! [`golden::check_index`]), then scores precision and recall per symbol.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use libsql::{Connection, params};
use serde::Deserialize;

use crate::golden::{self, IndexProblem};
use crate::symbols::SymbolKind;

/// A whole usage golden set, in file order. Only [`UsageSet::parse`] and
/// [`UsageSet::load`] build one, so every set is validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSet {
    symbols: Vec<UsageSymbol>,
}

/// One symbol and its true direct users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSymbol {
    pub fqn: String,
    pub kind: Option<SymbolKind>,
    pub users: Vec<String>,
    /// The methods overriding it; `None` when the file does not say.
    pub overridden_by: Option<Vec<String>>,
    pub note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    #[serde(rename = "symbol", default)]
    symbols: Vec<RawSymbol>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSymbol {
    fqn: String,
    #[serde(default)]
    kind: Option<String>,
    users: Vec<String>,
    #[serde(default)]
    overridden_by: Option<Vec<String>>,
    #[serde(default)]
    note: Option<String>,
}

impl UsageSet {
    /// Parse and validate a usage golden set from TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawSet = toml::from_str(text).context("parsing the usage golden set")?;
        Self::validate(raw)
    }

    /// Read, parse and validate the usage golden set at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading usage golden set {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in usage golden set {}", path.display()))
    }

    /// The symbols, in file order.
    pub fn symbols(&self) -> &[UsageSymbol] {
        &self.symbols
    }

    /// Reject an empty set, a repeated symbol, a malformed fqn, a user (or
    /// overrider) listed twice or equal to the symbol, a `kind` that is
    /// unknown or does not fit the fqn, and `overridden_by` on a type or
    /// constructor.
    fn validate(raw: RawSet) -> Result<Self> {
        if raw.symbols.is_empty() {
            bail!("the usage golden set has no symbols");
        }
        let mut seen = HashSet::new();
        let mut symbols = Vec::with_capacity(raw.symbols.len());
        for (index, symbol) in raw.symbols.into_iter().enumerate() {
            let number = index + 1;
            let fqn = &symbol.fqn;
            if !golden::is_fqn(fqn) {
                bail!("symbol {number}: {fqn:?} is not a fully qualified name");
            }
            if !seen.insert(fqn.clone()) {
                bail!("symbol {number} repeats {fqn}");
            }
            let is_method = fqn.contains('#') && !fqn.contains("#<init>(");
            if symbol.overridden_by.is_some() && !is_method {
                bail!("symbol {number}: only a method has overridden_by, {fqn} is none");
            }
            for (list, users) in [
                ("users", Some(&symbol.users)),
                ("overridden_by", symbol.overridden_by.as_ref()),
            ] {
                let mut distinct = HashSet::new();
                for user in users.into_iter().flatten() {
                    if !golden::is_fqn(user) {
                        bail!("symbol {number}: {user:?} in {list} is not a fully qualified name");
                    }
                    if user == fqn {
                        bail!("symbol {number} lists itself in {list}");
                    }
                    if !distinct.insert(user) {
                        bail!("symbol {number} lists {user} twice in {list}");
                    }
                }
            }
            let kind = match &symbol.kind {
                None => None,
                Some(name) => {
                    let kind = SymbolKind::parse(name).with_context(|| {
                        format!(
                            "symbol {number}: unknown kind {name:?} (expected one of {:?})",
                            SymbolKind::ALL.map(|kind| kind.as_str())
                        )
                    })?;
                    if !golden::kind_fits(kind, fqn) {
                        bail!(
                            "symbol {number}: {fqn} is not {}",
                            golden::with_article(kind.as_str())
                        );
                    }
                    Some(kind)
                }
            };
            symbols.push(UsageSymbol {
                fqn: symbol.fqn,
                kind,
                users: symbol.users,
                overridden_by: symbol.overridden_by,
                note: symbol.note,
            });
        }
        Ok(UsageSet { symbols })
    }
}

/// Every fqn of `set` (symbols, users, overriders) that the index on `conn`
/// does not hold, and every symbol whose stored kind differs from its
/// `kind`, in file order; `question` names the symbol entry (`symbol 2`).
pub async fn check_index(conn: &Connection, set: &UsageSet) -> Result<Vec<IndexProblem>> {
    let mut problems = Vec::new();
    for (number, symbol) in set.symbols.iter().enumerate() {
        let entry = format!("symbol {}", number + 1);
        let listed = std::iter::once(&symbol.fqn)
            .chain(&symbol.users)
            .chain(symbol.overridden_by.iter().flatten());
        for (position, fqn) in listed.enumerate() {
            let mut rows = conn
                .query("SELECT kind FROM symbols WHERE fqn = ?1", [fqn.as_str()])
                .await
                .with_context(|| format!("looking up {fqn}"))?;
            let Some(row) = rows.next().await? else {
                problems.push(IndexProblem::Missing {
                    question: entry.clone(),
                    fqn: fqn.clone(),
                });
                continue;
            };
            let actual: String = row.get(0)?;
            if let Some(expected) = symbol.kind.filter(|_| position == 0)
                && expected.as_str() != actual
            {
                problems.push(IndexProblem::WrongKind {
                    question: entry.clone(),
                    fqn: fqn.clone(),
                    expected: expected.as_str().to_string(),
                    actual,
                });
            }
        }
    }
    Ok(problems)
}

/// Expected against found: the hits, the expected ones not found, the
/// found ones not expected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Match {
    pub hits: usize,
    pub missed: Vec<String>,
    pub extra: Vec<String>,
}

impl Match {
    /// Compare `expected` with `found`; the lists come out sorted.
    pub fn of(expected: &[String], found: &[String]) -> Self {
        let expected: BTreeSet<&String> = expected.iter().collect();
        let found: BTreeSet<&String> = found.iter().collect();
        Self {
            hits: expected.intersection(&found).count(),
            missed: expected.difference(&found).map(|s| s.to_string()).collect(),
            extra: found.difference(&expected).map(|s| s.to_string()).collect(),
        }
    }

    pub fn expected(&self) -> usize {
        self.hits + self.missed.len()
    }

    pub fn found(&self) -> usize {
        self.hits + self.extra.len()
    }

    fn add(&mut self, other: &Match) {
        self.hits += other.hits;
        self.missed.extend(other.missed.iter().cloned());
        self.extra.extend(other.extra.iter().cloned());
    }
}

/// Whether a symbol is a type or a method or constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Types,
    Members,
}

impl Group {
    pub fn of(fqn: &str) -> Self {
        if fqn.contains('#') {
            Group::Members
        } else {
            Group::Types
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Group::Types => "types",
            Group::Members => "members",
        }
    }
}

/// How one symbol's users (and overriders) compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub fqn: String,
    pub group: Group,
    pub users: Match,
    /// `None` when the set does not list the overriders.
    pub overridden_by: Option<Match>,
}

/// Check `set` against the index on `conn` (any problem fails before
/// anything is scored), then compare every symbol's users and overriders
/// with the edges.
pub async fn evaluate(conn: &Connection, set: &UsageSet) -> Result<Vec<Outcome>> {
    let problems = check_index(conn, set).await?;
    if !problems.is_empty() {
        bail!(
            "the usage golden set does not match the index: {}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let mut outcomes = Vec::with_capacity(set.symbols.len());
    for symbol in &set.symbols {
        let users = users(conn, &symbol.fqn).await?;
        let overridden_by = match &symbol.overridden_by {
            Some(expected) => Some(Match::of(expected, &overriders(conn, &symbol.fqn).await?)),
            None => None,
        };
        outcomes.push(Outcome {
            fqn: symbol.fqn.clone(),
            group: Group::of(&symbol.fqn),
            users: Match::of(&symbol.users, &users),
            overridden_by,
        });
    }
    Ok(outcomes)
}

/// The direct users of `fqn` in the index: the sources of its incoming
/// edges other than `overrides`, and for a type those of its members and
/// nested types, without the sources inside it.
pub async fn users(conn: &Connection, fqn: &str) -> Result<Vec<String>> {
    fqns(
        conn,
        "WITH RECURSIVE inside(id) AS (
             SELECT id FROM symbols WHERE fqn = ?1
             UNION SELECT symbols.id FROM symbols JOIN inside ON symbols.parent_id = inside.id
         )
         SELECT DISTINCT src.fqn FROM edges
         JOIN symbols AS src ON src.id = edges.src_id
         WHERE edges.dst_id IN inside AND edges.src_id NOT IN inside
           AND edges.kind <> 'overrides'
         ORDER BY src.fqn",
        fqn,
    )
    .await
    .with_context(|| format!("reading the users of {fqn}"))
}

/// The methods with an `overrides` edge to `fqn`.
pub async fn overriders(conn: &Connection, fqn: &str) -> Result<Vec<String>> {
    fqns(
        conn,
        "SELECT DISTINCT src.fqn FROM edges
         JOIN symbols AS src ON src.id = edges.src_id
         JOIN symbols AS dst ON dst.id = edges.dst_id
         WHERE dst.fqn = ?1 AND edges.kind = 'overrides'
         ORDER BY src.fqn",
        fqn,
    )
    .await
    .with_context(|| format!("reading the overriders of {fqn}"))
}

async fn fqns(conn: &Connection, sql: &str, fqn: &str) -> Result<Vec<String>> {
    let mut rows = conn.query(sql, params![fqn]).await?;
    let mut found = Vec::new();
    while let Some(row) = rows.next().await? {
        found.push(row.get(0)?);
    }
    Ok(found)
}

/// `hits / found` and `hits / expected` as `P 0.750 R 1.000 (3/3, 4 found)`;
/// `-` for a ratio over nothing.
fn scores(found: &Match) -> String {
    let ratio = |part: usize, whole: usize| {
        if whole == 0 {
            "-".to_string()
        } else {
            format!("{:.3}", part as f64 / whole as f64)
        }
    };
    format!(
        "P {} R {} ({}/{}, {} found)",
        ratio(found.hits, found.found()),
        ratio(found.hits, found.expected()),
        found.hits,
        found.expected(),
        found.found()
    )
}

/// The outcomes, one line per symbol in set order with its missed and
/// extra users indented under it, then precision and recall over all
/// symbols, types and members (pooled: each symbol and user pair counts once), and over
/// the overriders:
///
/// ```text
/// 1. P 1.000 R 0.500 (1/2, 1 found) [members] com.acme.Consumer#consume(AddMessage)
///    missed com.acme.Dispatcher#retry(Message)
///    overridden by: P 1.000 R 1.000 (1/1, 1 found)
/// all 1: P 1.000 R 0.500 (1/2, 1 found)
/// types 0: P - R - (0/0, 0 found)
/// members 1: P 1.000 R 0.500 (1/2, 1 found)
/// overridden by 1: P 1.000 R 1.000 (1/1, 1 found)
/// ```
pub fn format_report(outcomes: &[Outcome]) -> String {
    let mut out = String::new();
    for (number, outcome) in outcomes.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} [{}] {}\n",
            number + 1,
            scores(&outcome.users),
            outcome.group.as_str(),
            outcome.fqn
        ));
        list(&mut out, "   ", &outcome.users);
        if let Some(overriders) = &outcome.overridden_by {
            out.push_str(&format!("   overridden by: {}\n", scores(overriders)));
            list(&mut out, "     ", overriders);
        }
    }
    let pooled = |group: Option<Group>| {
        let mut total = Match::default();
        let mut count = 0;
        for outcome in outcomes
            .iter()
            .filter(|outcome| group.is_none_or(|group| outcome.group == group))
        {
            total.add(&outcome.users);
            count += 1;
        }
        (count, total)
    };
    for (label, group) in [
        ("all", None),
        (Group::Types.as_str(), Some(Group::Types)),
        (Group::Members.as_str(), Some(Group::Members)),
    ] {
        let (count, total) = pooled(group);
        out.push_str(&format!("{label} {count}: {}\n", scores(&total)));
    }
    let mut overriders = Match::default();
    let mut count = 0;
    for found in outcomes.iter().filter_map(|o| o.overridden_by.as_ref()) {
        overriders.add(found);
        count += 1;
    }
    out.push_str(&format!("overridden by {count}: {}\n", scores(&overriders)));
    out
}

fn list(out: &mut String, indent: &str, found: &Match) {
    for fqn in &found.missed {
        out.push_str(&format!("{indent}missed {fqn}\n"));
    }
    for fqn in &found.extra {
        out.push_str(&format!("{indent}extra {fqn}\n"));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::store::{IndexReader, Store};

    fn error(text: &str) -> String {
        format!("{:#}", UsageSet::parse(text).unwrap_err())
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn loads_the_sample_fixture() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden/usages.toml");

        let set = UsageSet::load(&path).unwrap();

        assert!(set.symbols().len() >= 3);
        assert!(set.symbols().iter().any(|s| s.overridden_by.is_some()));
        assert!(set.symbols().iter().any(|s| s.kind.is_none()));
        assert!(set.symbols().iter().any(|s| !s.fqn.contains('#')));
    }

    #[test]
    fn rejects_empty_sets_repeats_self_users_and_misplaced_overriders() {
        assert!(error("").contains("no symbols"));
        let symbol =
            |fqn: &str, rest: &str| format!("[[symbol]]\nfqn = {fqn:?}\nusers = []\n{rest}\n");
        assert!(
            error(&format!("{}{}", symbol("a.B", ""), symbol("a.B", ""))).contains("repeats a.B")
        );
        assert!(error(&symbol("a.B#m(", "")).contains("not a fully qualified name"));
        assert!(
            error("[[symbol]]\nfqn = \"a.B\"\nusers = [\"a.C\", \"a.C\"]")
                .contains("lists a.C twice in users")
        );
        assert!(
            error("[[symbol]]\nfqn = \"a.B\"\nusers = [\"a.B\"]").contains("lists itself in users")
        );
        assert!(error("[[symbol]]\nfqn = \"a.B\"\nusers = [\"a C\"]").contains("in users"));
        assert!(
            error(&symbol("a.B", "overridden_by = [\"a.C#m()\"]"))
                .contains("only a method has overridden_by")
        );
        assert!(
            error(&symbol("a.B#<init>()", "overridden_by = []"))
                .contains("only a method has overridden_by")
        );
        assert!(
            error(&symbol(
                "a.B#m()",
                "overridden_by = [\"a.C#m()\", \"a.C#m()\"]"
            ))
            .contains("twice in overridden_by")
        );
        assert!(error(&symbol("a.B#m()", "kind = \"class\"")).contains("is not a class"));
        assert!(error(&symbol("a.B", "kind = \"type\"")).contains("unknown kind"));
        assert!(error("[[symbol]]\nfqn = \"a.B\"").contains("users"));
        assert!(error(&symbol("a.B", "used_by = []")).contains("used_by"));
    }

    #[test]
    fn match_counts_hits_misses_and_extras() {
        let found = Match::of(&strings(&["a", "b", "c"]), &strings(&["c", "d", "a"]));

        assert_eq!(found.hits, 2);
        assert_eq!(found.missed, ["b"]);
        assert_eq!(found.extra, ["d"]);
        assert_eq!((found.expected(), found.found()), (3, 3));
    }

    #[test]
    fn report_scores_each_symbol_and_pools_the_groups() {
        let outcomes = vec![
            Outcome {
                fqn: "a.Consumer#consume(Msg)".to_string(),
                group: Group::Members,
                users: Match::of(
                    &strings(&["a.D#run()", "a.D#retry()"]),
                    &strings(&["a.D#run()"]),
                ),
                overridden_by: Some(Match::of(&strings(&["a.L#consume(Msg)"]), &[])),
            },
            Outcome {
                fqn: "a.Consumer".to_string(),
                group: Group::Types,
                users: Match::of(&strings(&["a.D"]), &strings(&["a.D", "a.E"])),
                overridden_by: None,
            },
            Outcome {
                fqn: "a.Unused".to_string(),
                group: Group::Types,
                users: Match::default(),
                overridden_by: None,
            },
        ];

        assert_eq!(
            format_report(&outcomes),
            "\
1. P 1.000 R 0.500 (1/2, 1 found) [members] a.Consumer#consume(Msg)
   missed a.D#retry()
   overridden by: P - R 0.000 (0/1, 0 found)
     missed a.L#consume(Msg)
2. P 0.500 R 1.000 (1/1, 2 found) [types] a.Consumer
   extra a.E
3. P - R - (0/0, 0 found) [types] a.Unused
all 3: P 0.667 R 0.667 (2/3, 3 found)
types 2: P 0.500 R 1.000 (1/1, 2 found)
members 1: P 1.000 R 0.500 (1/2, 1 found)
overridden by 1: P - R 0.000 (0/1, 0 found)
"
        );
    }

    /// An index with `symbols` (fqn, kind, parent fqn) and `edges` (src,
    /// dst, kind).
    async fn index_with(
        symbols: &[(&str, &str, Option<&str>)],
        edges: &[(&str, &str, &str)],
    ) -> (tempfile::TempDir, IndexReader) {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        let conn = build.connection();
        for (fqn, kind, parent) in symbols {
            conn.execute(
                "INSERT INTO symbols (parent_id, kind, fqn, file, start_line, end_line, signature, content_hash) \
                 VALUES ((SELECT id FROM symbols WHERE fqn = ?3), ?1, ?2, 'A.java', 1, 1, '', '')",
                params![*kind, *fqn, *parent],
            )
            .await
            .unwrap();
        }
        for (line, (src, dst, kind)) in edges.iter().enumerate() {
            conn.execute(
                "INSERT INTO edges (src_id, dst_id, kind, line) VALUES \
                 ((SELECT id FROM symbols WHERE fqn = ?1), (SELECT id FROM symbols WHERE fqn = ?2), ?3, ?4)",
                params![*src, *dst, *kind, line as i64 + 1],
            )
            .await
            .unwrap();
        }
        build.commit().unwrap();
        drop(store);
        let reader = IndexReader::open(data.path()).await.unwrap();
        (data, reader)
    }

    #[tokio::test]
    async fn evaluate_rolls_up_a_type_and_scores_overriders_apart() {
        let (_data, reader) = index_with(
            &[
                ("a.Consumer", "interface", None),
                ("a.Consumer#consume(T)", "method", Some("a.Consumer")),
                ("a.Consumer#log()", "method", Some("a.Consumer")),
                ("a.Impl", "class", None),
                ("a.Impl#consume(Msg)", "method", Some("a.Impl")),
                ("a.Dispatcher", "class", None),
                ("a.Dispatcher#dispatch()", "method", Some("a.Dispatcher")),
                ("a.Dispatcher#retry()", "method", Some("a.Dispatcher")),
            ],
            &[
                ("a.Impl", "a.Consumer", "implements"),
                ("a.Impl#consume(Msg)", "a.Consumer#consume(T)", "overrides"),
                ("a.Dispatcher", "a.Consumer", "reference"),
                ("a.Dispatcher#dispatch()", "a.Consumer#consume(T)", "call"),
                ("a.Dispatcher#dispatch()", "a.Consumer#consume(T)", "call"),
                ("a.Consumer#log()", "a.Consumer#consume(T)", "call"),
                ("a.Consumer#log()", "a.Consumer", "reference"),
            ],
        )
        .await;
        let set = UsageSet::parse(
            r#"
[[symbol]]
fqn = "a.Consumer"
kind = "interface"
users = ["a.Impl", "a.Dispatcher", "a.Dispatcher#dispatch()", "a.Dispatcher#retry()"]

[[symbol]]
fqn = "a.Consumer#consume(T)"
users = ["a.Dispatcher#dispatch()", "a.Consumer#log()"]
overridden_by = ["a.Impl#consume(Msg)"]
"#,
        )
        .unwrap();

        let outcomes = evaluate(reader.connection(), &set).await.unwrap();

        assert_eq!(outcomes[0].group, Group::Types);
        assert_eq!(
            outcomes[0].users,
            Match {
                hits: 3,
                missed: strings(&["a.Dispatcher#retry()"]),
                extra: Vec::new(),
            }
        );
        assert_eq!(outcomes[0].overridden_by, None);
        assert_eq!(outcomes[1].users.hits, 2);
        assert!(outcomes[1].users.extra.is_empty());
        assert_eq!(
            outcomes[1].overridden_by,
            Some(Match {
                hits: 1,
                missed: Vec::new(),
                extra: Vec::new(),
            })
        );
    }

    #[tokio::test]
    async fn evaluate_fails_on_a_renamed_symbol_or_user_and_a_wrong_kind() {
        let (_data, reader) = index_with(
            &[("a.B", "class", None), ("a.B#m()", "method", Some("a.B"))],
            &[],
        )
        .await;
        let set = UsageSet::parse(
            r#"
[[symbol]]
fqn = "a.B"
kind = "interface"
users = ["a.C#gone()"]

[[symbol]]
fqn = "a.B#renamed()"
users = []
"#,
        )
        .unwrap();

        let problems = check_index(reader.connection(), &set).await.unwrap();
        let error = format!(
            "{:#}",
            evaluate(reader.connection(), &set).await.unwrap_err()
        );

        assert_eq!(problems.len(), 3);
        assert!(
            error.contains("a.B is a class, not an interface"),
            "{error}"
        );
        assert!(
            error.contains("a.C#gone() is not in the index (\"symbol 1\")"),
            "{error}"
        );
        assert!(
            error.contains("a.B#renamed() is not in the index (\"symbol 2\")"),
            "{error}"
        );
    }
}

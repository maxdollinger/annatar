//! The golden set: plain-language questions, each with the fully qualified
//! names of the symbols it should find, the fixed benchmark that retrieval
//! (5.3) is measured against.
//!
//! A golden set is a TOML file of `[[question]]` tables:
//!
//! ```toml
//! [[question]]
//! text = "Where do we list every user?"
//! expect = ["com.acme.sample.UserController#list()"]
//! kind = "method"
//! note = "the HTTP endpoint, not the service"
//! ```
//!
//! `expect` holds the symbol the question is about first, then optional
//! acceptable alternates; `kind` (optional) is the kind of that first symbol;
//! `note` is free text for the reader. Questions are written from the code and
//! the tickets, never by paraphrasing the generated descriptions, so the
//! benchmark does not reward the model for matching its own wording.
//! [`GoldenSet::parse`] validates the file (non-empty, distinct texts, fqn
//! shape without whitespace, kind consistent with the fqn) and
//! [`check_index`] reports expected symbols an index does not hold (a renamed
//! symbol), so a stale question fails loudly instead of counting as a
//! retrieval miss.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use libsql::Connection;
use regex::Regex;
use serde::Deserialize;

use crate::summaries::collapse_whitespace;
use crate::symbols::SymbolKind;

/// A whole golden set, in file order. Only [`GoldenSet::parse`] and
/// [`GoldenSet::load`] build one, so every set is validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenSet {
    questions: Vec<Question>,
}

/// One question and the symbols that answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    text: String,
    expect: Vec<String>,
    kind: Option<SymbolKind>,
    note: Option<String>,
}

/// The file as written, before validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    #[serde(rename = "question", default)]
    questions: Vec<RawQuestion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuestion {
    text: String,
    expect: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

impl Question {
    /// The question as a developer would ask it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The fqn the question is about, then acceptable alternates (never empty).
    pub fn expect(&self) -> &[String] {
        &self.expect
    }

    /// The symbol the question is about.
    pub fn primary(&self) -> &str {
        &self.expect[0]
    }

    /// The kind of the first expected symbol, when the file names it.
    pub fn kind(&self) -> Option<SymbolKind> {
        self.kind
    }

    /// Free text for the reader (why these symbols, what makes it hard).
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// A type fqn (`com.acme.Orders.Line`) or a member fqn (`com.acme.Orders#find(Long)`,
/// `com.acme.Orders#<init>()`), parameter types as the index stores them:
/// as written, without whitespace.
static FQN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)*(#(<init>|[A-Za-z_$][\w$]*)\([\w$.<>,\[\]?]*\))?$",
    )
    .expect("the fqn pattern is valid")
});

/// `kind` with its indefinite article ("a class", "an interface").
fn with_article(kind: &str) -> String {
    let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {kind}")
}

impl GoldenSet {
    /// Parse and validate a golden set from TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawSet = toml::from_str(text).context("parsing the golden set")?;
        Self::validate(raw)
    }

    /// Read, parse and validate the golden set at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading golden set {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in golden set {}", path.display()))
    }

    /// The questions, in file order.
    pub fn questions(&self) -> &[Question] {
        &self.questions
    }

    /// Reject an empty set, a blank or repeated question (compared ignoring
    /// case and whitespace), a question without expected symbols, a
    /// malformed (including any whitespace) or repeated fqn, and a `kind`
    /// that is unknown or does not fit the first fqn (a member has `#`, a
    /// constructor `#<init>(`).
    fn validate(raw: RawSet) -> Result<Self> {
        if raw.questions.is_empty() {
            anyhow::bail!("the golden set has no questions");
        }
        let mut texts = HashSet::new();
        let mut questions = Vec::with_capacity(raw.questions.len());
        for (index, question) in raw.questions.into_iter().enumerate() {
            let number = index + 1;
            let text = collapse_whitespace(&question.text).to_lowercase();
            if text.is_empty() {
                anyhow::bail!("question {number} has no text");
            }
            if !texts.insert(text) {
                anyhow::bail!("question {number} repeats an earlier question");
            }
            let Some(primary) = question.expect.first() else {
                anyhow::bail!("question {number} expects no symbol");
            };
            let mut fqns = HashSet::new();
            for fqn in &question.expect {
                if !FQN.is_match(fqn) {
                    anyhow::bail!("question {number}: {fqn:?} is not a fully qualified name");
                }
                if !fqns.insert(fqn) {
                    anyhow::bail!("question {number} expects {fqn} twice");
                }
            }
            let kind = match &question.kind {
                None => None,
                Some(name) => {
                    let kind = SymbolKind::parse(name).with_context(|| {
                        format!(
                            "question {number}: unknown kind {name:?} (expected one of {:?})",
                            SymbolKind::ALL.map(|kind| kind.as_str())
                        )
                    })?;
                    let fits = match kind {
                        SymbolKind::Constructor => primary.contains("#<init>("),
                        SymbolKind::Method => {
                            primary.contains('#') && !primary.contains("#<init>(")
                        }
                        _ => !primary.contains('#'),
                    };
                    if !fits {
                        anyhow::bail!(
                            "question {number}: {primary} is not {}",
                            with_article(kind.as_str())
                        );
                    }
                    Some(kind)
                }
            };
            questions.push(Question {
                text: question.text,
                expect: question.expect,
                kind,
                note: question.note,
            });
        }
        Ok(GoldenSet { questions })
    }
}

/// An expected symbol that does not match the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexProblem {
    /// No symbol has this fqn.
    Missing { question: String, fqn: String },
    /// The first expected symbol exists with another kind.
    WrongKind {
        question: String,
        fqn: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for IndexProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexProblem::Missing { question, fqn } => {
                write!(f, "{fqn} is not in the index ({question:?})")
            }
            IndexProblem::WrongKind {
                question,
                fqn,
                expected,
                actual,
            } => write!(
                f,
                "{fqn} is {}, not {} ({question:?})",
                with_article(actual),
                with_article(expected)
            ),
        }
    }
}

/// Every expected fqn of `set` that the index on `conn` does not hold, and
/// every first expected symbol whose stored kind differs from the question's
/// `kind`, in question order.
pub async fn check_index(conn: &Connection, set: &GoldenSet) -> Result<Vec<IndexProblem>> {
    let mut problems = Vec::new();
    for question in &set.questions {
        for (position, fqn) in question.expect.iter().enumerate() {
            let mut rows = conn
                .query("SELECT kind FROM symbols WHERE fqn = ?1", [fqn.as_str()])
                .await
                .with_context(|| format!("looking up {fqn}"))?;
            let Some(row) = rows.next().await? else {
                problems.push(IndexProblem::Missing {
                    question: question.text.clone(),
                    fqn: fqn.clone(),
                });
                continue;
            };
            let actual: String = row.get(0)?;
            if let Some(expected) = question.kind.filter(|_| position == 0)
                && expected.as_str() != actual
            {
                problems.push(IndexProblem::WrongKind {
                    question: question.text.clone(),
                    fqn: fqn.clone(),
                    expected: expected.as_str().to_string(),
                    actual,
                });
            }
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::store::IndexReader;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden/sample.toml")
    }

    fn error(text: &str) -> String {
        format!("{:#}", GoldenSet::parse(text).unwrap_err())
    }

    #[test]
    fn loads_the_sample_fixture() {
        let set = GoldenSet::load(&fixture()).unwrap();

        assert_eq!(set.questions().len(), 5);
        let first = &set.questions()[0];
        assert_eq!(first.primary(), "com.acme.sample.UserController#list()");
        assert_eq!(first.kind(), Some(SymbolKind::Method));
        assert!(!first.text().is_empty());
        let alternates = set
            .questions()
            .iter()
            .find(|question| question.expect().len() > 1)
            .expect("a question with an alternate");
        assert!(alternates.note().is_some());
        assert!(
            set.questions()
                .iter()
                .any(|question| question.kind().is_none())
        );
    }

    #[test]
    fn accepts_member_type_and_nested_fqns() {
        let set = GoldenSet::parse(
            r#"
[[question]]
text = "a"
expect = [
  "com.acme.Orders",
  "com.acme.Orders.Line",
  "com.acme.Orders#find(Long)",
  "com.acme.Orders#<init>()",
  "com.acme.Orders#put(Map<UUID,Set<String>>,JwtUtils.JwtInfo,byte[],List<?>)",
  "Default",
]
"#,
        )
        .unwrap();

        assert_eq!(set.questions()[0].expect().len(), 6);
    }

    #[test]
    fn rejects_empty_sets_blank_and_repeated_questions() {
        assert!(error("").contains("no questions"));
        assert!(
            error("[[question]]\ntext = \"  \"\nexpect = [\"a.B\"]")
                .contains("question 1 has no text")
        );
        let repeated = "
[[question]]
text = \"Where do we  list users?\"
expect = [\"a.B\"]

[[question]]
text = \"where do we list users?\"
expect = [\"a.C\"]
";
        assert!(error(repeated).contains("question 2 repeats"));
        assert!(error("[[question]]\ntext = \"q\"\nexpect = []").contains("expects no symbol"));
    }

    #[test]
    fn rejects_malformed_and_repeated_fqns_and_unknown_fields() {
        for fqn in [
            "",
            "com.acme.",
            "com acme.B",
            "com.acme.B#find",
            "com.acme.B#find(Long",
            "com.acme.B:12",
            "com.acme.B#a()#b()",
            "com.acme.B#put(Map<UUID, Set<String>>)",
            "com.acme.B#find( Long)",
            " com.acme.B",
        ] {
            let text = format!("[[question]]\ntext = \"q\"\nexpect = [{fqn:?}]");
            assert!(
                error(&text).contains("is not a fully qualified name"),
                "{fqn:?}: {}",
                error(&text)
            );
        }
        assert!(
            error("[[question]]\ntext = \"q\"\nexpect = [\"a.B\", \"a.B\"]")
                .contains("expects a.B twice")
        );
        assert!(
            error("[[question]]\ntext = \"q\"\nexpect = [\"a.B\"]\nanswer = \"x\"")
                .contains("answer")
        );
    }

    #[test]
    fn kind_must_be_known_and_fit_the_first_fqn() {
        let with = |kind: &str, fqn: &str| {
            GoldenSet::parse(&format!(
                "[[question]]\ntext = \"q\"\nexpect = [{fqn:?}, \"a.Other#x()\"]\nkind = {kind:?}"
            ))
        };

        assert!(with("class", "a.B").is_ok());
        assert!(with("enum", "a.B.Kind").is_ok());
        assert!(with("method", "a.B#find(Long)").is_ok());
        assert!(with("constructor", "a.B#<init>(Long)").is_ok());
        assert!(
            format!("{:#}", with("class", "a.B#find()").unwrap_err()).contains("is not a class")
        );
        assert!(format!("{:#}", with("method", "a.B").unwrap_err()).contains("is not a method"));
        assert!(
            format!("{:#}", with("method", "a.B#<init>()").unwrap_err())
                .contains("is not a method")
        );
        assert!(
            format!("{:#}", with("interface", "a.B#b()").unwrap_err())
                .contains("is not an interface")
        );
        assert!(
            format!("{:#}", with("constructor", "a.B#b()").unwrap_err())
                .contains("is not a constructor")
        );
        assert!(
            format!("{:#}", with("type", "a.B").unwrap_err()).contains("unknown kind \"type\"")
        );
    }

    async fn index_with(symbols: &[(&str, &str)]) -> (tempfile::TempDir, IndexReader) {
        let data = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(data.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        for (fqn, kind) in symbols {
            build
                .connection()
                .execute(
                    "INSERT INTO symbols (kind, fqn, file, start_line, end_line, signature, content_hash) \
                     VALUES (?1, ?2, 'A.java', 1, 1, '', '')",
                    [*kind, *fqn],
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
    async fn check_reports_missing_fqns_and_a_wrong_kind() {
        let (_data, reader) = index_with(&[
            ("a.B", "class"),
            ("a.B#find(Long)", "method"),
            ("a.C", "interface"),
        ])
        .await;
        let set = GoldenSet::parse(
            r#"
[[question]]
text = "found"
expect = ["a.B#find(Long)", "a.B"]
kind = "method"

[[question]]
text = "renamed"
expect = ["a.B#lookup(Long)", "a.B", "a.Gone"]

[[question]]
text = "wrong kind"
expect = ["a.C", "a.B"]
kind = "class"
"#,
        )
        .unwrap();

        let problems = check_index(reader.connection(), &set).await.unwrap();

        assert_eq!(
            problems,
            vec![
                IndexProblem::Missing {
                    question: "renamed".to_string(),
                    fqn: "a.B#lookup(Long)".to_string(),
                },
                IndexProblem::Missing {
                    question: "renamed".to_string(),
                    fqn: "a.Gone".to_string(),
                },
                IndexProblem::WrongKind {
                    question: "wrong kind".to_string(),
                    fqn: "a.C".to_string(),
                    expected: "class".to_string(),
                    actual: "interface".to_string(),
                },
            ]
        );
        assert_eq!(
            problems[2].to_string(),
            "a.C is an interface, not a class (\"wrong kind\")"
        );
    }

    /// Checks a real golden set against a real index: `ANNATAR_GOLDEN_SET`
    /// (the set's TOML file, kept out of git) and `ANNATAR_GOLDEN_DATA_DIR`
    /// (the data directory holding `index.db`, from a full run without
    /// `--path`).
    #[tokio::test]
    #[ignore = "needs a real golden set and a full index"]
    async fn real_golden_set_matches_the_real_index() {
        let set_path = std::env::var_os("ANNATAR_GOLDEN_SET")
            .map(PathBuf::from)
            .expect("set ANNATAR_GOLDEN_SET to a golden set file");
        let data_dir = std::env::var_os("ANNATAR_GOLDEN_DATA_DIR")
            .map(PathBuf::from)
            .expect("set ANNATAR_GOLDEN_DATA_DIR to the data directory of a full index");
        let set = GoldenSet::load(&set_path).unwrap();
        let reader = IndexReader::open(&data_dir).await.unwrap();

        let problems = check_index(reader.connection(), &set).await.unwrap();

        let fqns: usize = set
            .questions()
            .iter()
            .map(|question| question.expect().len())
            .sum();
        println!(
            "{} questions, {fqns} expected fqns, {} problems",
            set.questions().len(),
            problems.len()
        );
        assert!(
            problems.is_empty(),
            "{}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

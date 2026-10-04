//! Retrieval quality against a golden set: `annatar eval` (5.3).
//!
//! Every question of a [`GoldenSet`] runs through [`search::search`] without
//! a filter, exactly as `annatar search "<question>"` would, and is scored by
//! the rank of its first expected fqn (the primary) and by the best rank of
//! any expected fqn (the primary or an alternate). The summary counts top-1
//! and top-5 hits and the mean reciprocal rank, for all questions and for
//! types and members (methods, constructors) apart. [`golden::check_index`]
//! runs first, so an expected symbol the index does not hold fails the
//! evaluation instead of counting as a miss.

use anyhow::{Result, bail};
use libsql::Connection;

use crate::golden::{self, GoldenSet, Question};
use crate::schema;
use crate::search::{self, Filter, QueryEmbedder};

/// Whether a question asks for a type or for a method or constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Types,
    Members,
}

impl Group {
    /// The group of `question`'s primary fqn: a member has `#`.
    pub fn of(question: &Question) -> Self {
        if question.primary().contains('#') {
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

/// How one question ranked; ranks count from 1, `None` is not in the hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub group: Group,
    pub primary: String,
    /// Rank of the first expected fqn.
    pub primary_rank: Option<usize>,
    /// Best rank of any expected fqn.
    pub any_rank: Option<usize>,
    /// The first hit, when it is not the primary.
    pub top: Option<String>,
}

/// Score `question` against `hits`, the fqns in rank order.
pub fn score(question: &Question, hits: &[String]) -> Outcome {
    let rank = |fqn: &str| hits.iter().position(|hit| hit == fqn).map(|i| i + 1);
    let primary_rank = rank(question.primary());
    let any_rank = question.expect().iter().filter_map(|fqn| rank(fqn)).min();
    Outcome {
        group: Group::of(question),
        primary: question.primary().to_string(),
        primary_rank,
        any_rank,
        top: hits
            .first()
            .filter(|top| top.as_str() != question.primary())
            .cloned(),
    }
}

/// Top-1, top-5 and mean reciprocal rank of one way of ranking.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scores {
    pub top1: usize,
    pub top5: usize,
    /// Mean of 1 / rank, a miss counting 0.
    pub mrr: f64,
}

impl Scores {
    fn of(ranks: &[Option<usize>]) -> Self {
        if ranks.is_empty() {
            return Self::default();
        }
        Self {
            top1: ranks.iter().filter(|rank| **rank == Some(1)).count(),
            top5: ranks
                .iter()
                .filter(|rank| rank.is_some_and(|r| r <= 5))
                .count(),
            mrr: ranks
                .iter()
                .map(|rank| rank.map_or(0.0, |r| 1.0 / r as f64))
                .sum::<f64>()
                / ranks.len() as f64,
        }
    }
}

/// The scores of a group of questions.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// `all`, `types` or `members`.
    pub label: &'static str,
    pub questions: usize,
    pub primary: Scores,
    pub any: Scores,
}

/// One summary for all outcomes, then one for types and one for members.
pub fn summarize(outcomes: &[Outcome]) -> Vec<Summary> {
    let summary = |label, group: Option<Group>| {
        let chosen: Vec<&Outcome> = outcomes
            .iter()
            .filter(|outcome| group.is_none_or(|group| outcome.group == group))
            .collect();
        let ranks = |pick: fn(&Outcome) -> Option<usize>| {
            chosen
                .iter()
                .map(|outcome| pick(outcome))
                .collect::<Vec<_>>()
        };
        Summary {
            label,
            questions: chosen.len(),
            primary: Scores::of(&ranks(|outcome| outcome.primary_rank)),
            any: Scores::of(&ranks(|outcome| outcome.any_rank)),
        }
    };
    vec![
        summary("all", None),
        summary(Group::Types.as_str(), Some(Group::Types)),
        summary(Group::Members.as_str(), Some(Group::Members)),
    ]
}

/// Check `set` against the index on `conn`, then search every question with
/// `limit` hits and score it. Any [`golden::IndexProblem`] fails before the
/// first search.
pub async fn evaluate(
    conn: &Connection,
    embedder: &QueryEmbedder,
    set: &GoldenSet,
    limit: usize,
) -> Result<Vec<Outcome>> {
    let problems = golden::check_index(conn, set).await?;
    if !problems.is_empty() {
        bail!(
            "the golden set does not match the index: {}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let mut outcomes = Vec::with_capacity(set.questions().len());
    for question in set.questions() {
        let hits = search::search(conn, embedder, question.text(), &Filter::default(), limit)
            .await?
            .into_iter()
            .map(|hit| hit.fqn)
            .collect::<Vec<_>>();
        outcomes.push(score(question, &hits));
    }
    Ok(outcomes)
}

/// The index's embedding settings and the hits searched per question as one
/// line, so reports of two variants say which is which (MRR counts only
/// ranks within `limit`):
/// `eval: embedding_model=bge-m3 dim=1024 parent_description=false k=10`.
pub async fn settings_line(conn: &Connection, limit: usize) -> Result<String> {
    let meta = search::embedding_meta(conn).await?;
    let parent = search::meta_value(conn, schema::META_EMBEDDING_PARENT_DESCRIPTION)
        .await?
        .unwrap_or_else(|| "unknown".to_string());
    Ok(format!(
        "eval: embedding_model={} dim={} parent_description={parent} k={limit}\n",
        meta.model, meta.dim
    ))
}

/// The outcomes, one line per question in set order, then the summaries:
///
/// ```text
/// 1. 1 1 [members] com.acme.UserService#find(Long)
/// 2. 3 1 [types] com.acme.UserService top=com.acme.UserController
/// 3. - - [types] com.acme.User top=com.acme.UserService
/// all 3: primary top-1 1 top-5 2 mrr 0.444; any top-1 2 top-5 2 mrr 0.667
/// types 2: …
/// members 1: …
/// ```
///
/// question number, primary rank, best rank of any expected fqn (`-` when
/// not within the hits), the group, the primary fqn and, when it is not
/// first, the first hit.
pub fn format_report(outcomes: &[Outcome]) -> String {
    let rank = |rank: Option<usize>| rank.map_or("-".to_string(), |rank| rank.to_string());
    let mut out = String::new();
    for (number, outcome) in outcomes.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} {} [{}] {}",
            number + 1,
            rank(outcome.primary_rank),
            rank(outcome.any_rank),
            outcome.group.as_str(),
            outcome.primary
        ));
        if let Some(top) = &outcome.top {
            out.push_str(&format!(" top={top}"));
        }
        out.push('\n');
    }
    for summary in summarize(outcomes) {
        let scores = |scores: Scores| {
            format!(
                "top-1 {} top-5 {} mrr {:.3}",
                scores.top1, scores.top5, scores.mrr
            )
        };
        out.push_str(&format!(
            "{} {}: primary {}; any {}\n",
            summary.label,
            summary.questions,
            scores(summary.primary),
            scores(summary.any)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use libsql::params;

    use super::*;
    use crate::config::OllamaConfig;
    use crate::llm::fake::FakeBackend;
    use crate::store::{IndexReader, Store};

    fn set(text: &str) -> GoldenSet {
        GoldenSet::parse(text).unwrap()
    }

    fn hits(fqns: &[&str]) -> Vec<String> {
        fqns.iter().map(|fqn| fqn.to_string()).collect()
    }

    const SET: &str = r#"
[[question]]
text = "first"
expect = ["a.B#find(Long)", "a.B"]
kind = "method"

[[question]]
text = "second"
expect = ["a.C", "a.C#run()", "a.D"]

[[question]]
text = "third"
expect = ["a.E"]
"#;

    #[test]
    fn score_ranks_the_primary_and_the_best_of_any() {
        let set = set(SET);
        let [first, second, third] = set.questions() else {
            panic!("three questions");
        };

        assert_eq!(
            score(first, &hits(&["a.B#find(Long)", "a.B"])),
            Outcome {
                group: Group::Members,
                primary: "a.B#find(Long)".to_string(),
                primary_rank: Some(1),
                any_rank: Some(1),
                top: None,
            }
        );
        assert_eq!(
            score(second, &hits(&["a.X", "a.D", "a.C#run()", "a.Y", "a.C"])),
            Outcome {
                group: Group::Types,
                primary: "a.C".to_string(),
                primary_rank: Some(5),
                any_rank: Some(2),
                top: Some("a.X".to_string()),
            }
        );
        let miss = score(third, &hits(&["a.X"]));
        assert_eq!((miss.primary_rank, miss.any_rank), (None, None));
        assert_eq!(score(third, &[]).top, None);
    }

    fn outcome(group: Group, primary: Option<usize>, any: Option<usize>) -> Outcome {
        Outcome {
            group,
            primary: "a.B".to_string(),
            primary_rank: primary,
            any_rank: any,
            top: None,
        }
    }

    #[test]
    fn summaries_count_top_1_top_5_and_mrr_per_group() {
        let outcomes = [
            outcome(Group::Types, Some(1), Some(1)),
            outcome(Group::Types, Some(6), Some(2)),
            outcome(Group::Members, Some(4), Some(1)),
            outcome(Group::Members, None, None),
        ];

        let summaries = summarize(&outcomes);

        assert_eq!(
            summaries
                .iter()
                .map(|summary| (summary.label, summary.questions))
                .collect::<Vec<_>>(),
            [("all", 4), ("types", 2), ("members", 2)]
        );
        let all = &summaries[0];
        assert_eq!((all.primary.top1, all.primary.top5), (1, 2));
        assert!((all.primary.mrr - (1.0 + 1.0 / 6.0 + 0.25) / 4.0).abs() < 1e-9);
        assert_eq!((all.any.top1, all.any.top5), (2, 3));
        assert!((all.any.mrr - 2.5 / 4.0).abs() < 1e-9);
        assert_eq!((summaries[1].primary.top1, summaries[1].any.top5), (1, 2));
        assert_eq!((summaries[2].primary.top5, summaries[2].any.top1), (1, 1));
        assert_eq!(summarize(&[])[1].primary, Scores::default());
    }

    #[test]
    fn report_has_a_line_per_question_and_per_group() {
        let mut outcomes = vec![
            outcome(Group::Members, Some(1), Some(1)),
            outcome(Group::Types, Some(3), Some(1)),
            outcome(Group::Types, None, None),
        ];
        outcomes[0].primary = "a.B#find(Long)".to_string();
        outcomes[1].top = Some("a.X".to_string());
        outcomes[2].top = Some("a.Y".to_string());

        assert_eq!(
            format_report(&outcomes),
            "\
1. 1 1 [members] a.B#find(Long)
2. 3 1 [types] a.B top=a.X
3. - - [types] a.B top=a.Y
all 3: primary top-1 1 top-5 2 mrr 0.444; any top-1 2 top-5 2 mrr 0.667
types 2: primary top-1 0 top-5 1 mrr 0.167; any top-1 1 top-5 1 mrr 0.500
members 1: primary top-1 1 top-5 1 mrr 1.000; any top-1 1 top-5 1 mrr 1.000
"
        );
    }

    /// An index of `rows` (fqn, kind, vector) with 3-dimensional vectors.
    async fn index(rows: &[(&str, &str, [f32; 3])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        let conn = build.connection();
        schema::create_vectors(conn, 3).await.unwrap();
        for (id, (fqn, kind, vector)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO symbols (id, kind, fqn, file, start_line, end_line, signature, content_hash)
                 VALUES (?1, ?2, ?3, 'A.java', 1, 1, '', '')",
                params![id as i64 + 1, *kind, *fqn],
            )
            .await
            .unwrap();
            let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO symbol_vectors (symbol_id, embedding) VALUES (?1, vector32(?2))",
                params![id as i64 + 1, blob],
            )
            .await
            .unwrap();
        }
        for (key, value) in [
            (schema::META_EMBEDDING_MODEL, "embed"),
            (schema::META_EMBEDDING_DIM, "3"),
        ] {
            conn.execute(
                "INSERT INTO index_meta (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .await
            .unwrap();
        }
        build.commit().unwrap();
        dir
    }

    #[tokio::test]
    async fn settings_line_names_the_embedding_settings_and_k() {
        let data = index(&[("a.Near", "class", [2.0, 97.0, 1.0])]).await;
        let reader = IndexReader::open(data.path()).await.unwrap();

        assert_eq!(
            settings_line(reader.connection(), 7).await.unwrap(),
            "eval: embedding_model=embed dim=3 parent_description=unknown k=7\n"
        );
    }

    async fn embedder(backend: Arc<FakeBackend>) -> QueryEmbedder {
        let config = OllamaConfig {
            url: "http://localhost:11434".to_string(),
            chat_model: "chat".to_string(),
            embedding_model: "embed".to_string(),
            reasoning_effort: "none".to_string(),
            temperature: 0.0,
            max_tokens: crate::config::DEFAULT_MAX_TOKENS,
        };
        QueryEmbedder::new(backend, &config).await.unwrap()
    }

    #[tokio::test]
    async fn evaluate_searches_every_question_like_search() {
        // The fake embeds a text as [chars, first byte, 1]: "aa" = [2, 97, 1],
        // "bbbbbbbbbb" = [10, 98, 1].
        let data = index(&[
            ("a.Near", "class", [2.0, 97.0, 1.0]),
            ("a.Near#run()", "method", [2.0, 90.0, 1.0]),
            ("a.Far", "class", [90.0, 2.0, 1.0]),
        ])
        .await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let backend = FakeBackend::new();
        let embedder = embedder(backend.clone()).await;
        let set = set(r#"
[[question]]
text = "aa"
expect = ["a.Near#run()", "a.Near"]

[[question]]
text = "bbbbbbbbbb"
expect = ["a.Far"]
"#);

        let outcomes = evaluate(reader.connection(), &embedder, &set, 2)
            .await
            .unwrap();

        assert_eq!(
            backend.batches(),
            [vec!["aa".to_string()], vec!["bbbbbbbbbb".to_string()]]
        );
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| (outcome.primary_rank, outcome.any_rank))
                .collect::<Vec<_>>(),
            [(Some(2), Some(1)), (None, None)]
        );
        assert_eq!(outcomes[0].top.as_deref(), Some("a.Near"));
    }

    #[tokio::test]
    async fn evaluate_fails_on_a_symbol_missing_from_the_index() {
        let data = index(&[("a.Near", "class", [2.0, 97.0, 1.0])]).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let backend = FakeBackend::new();
        let embedder = embedder(backend.clone()).await;
        let set = set("[[question]]\ntext = \"aa\"\nexpect = [\"a.Near\", \"a.Gone\"]\n");

        let err = evaluate(reader.connection(), &embedder, &set, 5)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "the golden set does not match the index: a.Gone is not in the index (\"aa\")"
        );
        assert!(backend.batches().is_empty());
    }
}

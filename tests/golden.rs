use std::path::PathBuf;

use annatar::config::DEFAULT_TICKET_REGEX;
use annatar::golden::{GoldenSet, check_index};
use annatar::indexer;
use annatar::store::{IndexReader, Store};
use annatar::tickets::JiraMode;
use annatar::usage_eval::{self, UsageSet};
use regex::Regex;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[tokio::test]
async fn sample_golden_set_matches_the_sample_index() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).await.unwrap();
    indexer::build_index(
        &store,
        &fixtures().join("sample-project"),
        None,
        &Regex::new(DEFAULT_TICKET_REGEX).unwrap(),
        &JiraMode::Disabled,
        None,
    )
    .await
    .unwrap();
    drop(store);
    let set = GoldenSet::load(&fixtures().join("golden/sample.toml")).unwrap();
    let reader = IndexReader::open(data.path()).await.unwrap();

    let problems = check_index(reader.connection(), &set).await.unwrap();

    assert!(problems.is_empty(), "{problems:?}");
}

#[tokio::test]
async fn sample_usage_set_matches_the_sample_index_edges() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).await.unwrap();
    indexer::build_index(
        &store,
        &fixtures().join("sample-project"),
        None,
        &Regex::new(DEFAULT_TICKET_REGEX).unwrap(),
        &JiraMode::Disabled,
        None,
    )
    .await
    .unwrap();
    drop(store);
    let set = UsageSet::load(&fixtures().join("golden/usages.toml")).unwrap();
    let reader = IndexReader::open(data.path()).await.unwrap();

    let outcomes = usage_eval::evaluate(reader.connection(), &set)
        .await
        .unwrap();

    let report = usage_eval::format_report(&outcomes);
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.users.missed.is_empty()
                && outcome.users.extra.is_empty()
                && outcome
                    .overridden_by
                    .as_ref()
                    .is_none_or(|found| found.missed.is_empty() && found.extra.is_empty())),
        "{report}"
    );
    assert!(
        report.contains("all 4: P 1.000 R 1.000 (12/12, 12 found)"),
        "{report}"
    );
}

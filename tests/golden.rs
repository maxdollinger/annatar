use std::path::PathBuf;

use annatar::config::DEFAULT_TICKET_REGEX;
use annatar::golden::{GoldenSet, check_index};
use annatar::indexer;
use annatar::store::{IndexReader, Store};
use annatar::tickets::JiraMode;
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

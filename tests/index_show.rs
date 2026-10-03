use std::path::PathBuf;

use annatar::indexer;
use annatar::show;
use annatar::store::{IndexReader, Store};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-project")
}

#[tokio::test]
async fn sample_project_indexes_and_show_lists_children() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).await.unwrap();

    let stats = indexer::build_index(&store, &fixture(), None)
        .await
        .unwrap();
    assert_eq!(stats.files, 4, "one file per sample type");
    assert_eq!(stats.empty, 0);
    assert_eq!(stats.parse_errors, 0);
    assert_eq!(stats.unreadable, 0);
    assert_eq!(stats.symbols, 14);

    drop(store);

    let reader = IndexReader::open(data.path()).await.unwrap();
    let conn = reader.connection();

    let controller = show::render(conn, "com.acme.sample.UserController")
        .await
        .unwrap();
    assert!(
        controller.contains("com.acme.sample.UserController [class] role=controller"),
        "controller role and kind: {controller}"
    );
    assert!(controller.contains("- annotations: @RestController, @RequestMapping(\"/users\")"));
    assert!(controller.contains("- javadoc:\n    Serves users over HTTP."));
    assert!(
        controller.contains("com.acme.sample.UserController#<init>(UserService) [constructor]")
    );
    assert!(controller.contains("com.acme.sample.UserController#list() [method]"));
    assert!(controller.contains("com.acme.sample.UserController#get(Long) [method]"));
    assert!(
        controller.contains("com.acme.sample.UserController.ErrorResponse [class]"),
        "nested type listed as a child: {controller}"
    );

    let repository = show::render(conn, "com.acme.sample.UserRepository")
        .await
        .unwrap();
    assert!(
        repository.contains("com.acme.sample.UserRepository [interface] role=repository"),
        "Spring Data repository role: {repository}"
    );
    assert!(repository.contains("com.acme.sample.UserRepository#findByEmail(String) [method]"));

    let service = show::render(conn, "com.acme.sample.UserService")
        .await
        .unwrap();
    assert!(service.contains("com.acme.sample.UserService#find(Long) [method]"));
    assert!(service.contains("com.acme.sample.UserService#find(String) [method]"));
}

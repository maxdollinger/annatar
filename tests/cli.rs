use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-project")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the sample project (not a git work tree) with a config whose
/// Ollama server is unreachable.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), &dir.path().join("repo"));
    std::fs::write(
        dir.path().join("annatar.toml"),
        "repo = \"repo\"\ndata_dir = \"data\"\n\n[ollama]\nurl = \"http://127.0.0.1:9\"\nchat_model = \"chat\"\nembedding_model = \"embed\"\n",
    )
    .unwrap();
    dir
}

fn annatar(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_annatar"))
        .current_dir(dir)
        .env_remove("RUST_LOG")
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn results_go_to_stdout_logs_and_errors_to_stderr() {
    let dir = workspace();

    let index = annatar(dir.path(), &["-vv", "index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    let stdout = text(&index.stdout);
    assert!(
        stdout.starts_with("indexed 4 files, 14 symbols"),
        "{stdout}"
    );
    assert!(
        stdout.lines().all(|line| !line.contains("INFO")
            && !line.contains("WARN")
            && !line.contains("DEBUG")),
        "logs on stdout: {stdout}"
    );
    let stderr = text(&index.stderr);
    assert!(
        stderr.contains("DEBUG") && stderr.contains("WARN"),
        "{stderr}"
    );
    assert!(
        !stderr.contains('\u{1b}'),
        "no ANSI codes off a terminal: {stderr}"
    );

    let show = annatar(
        dir.path(),
        &["-vv", "show", "com.acme.sample.UserService#find(Long)"],
    );
    assert!(show.status.success(), "{}", text(&show.stderr));
    let stdout = text(&show.stdout);
    assert!(
        stdout.starts_with("com.acme.sample.UserService#find(Long) [method]\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  - parent: com.acme.sample.UserService\n"),
        "{stdout}"
    );
    assert!(text(&show.stderr).contains("DEBUG"));

    let search = annatar(dir.path(), &["search", "who loads users"]);
    assert_eq!(search.status.code(), Some(1));
    assert_eq!(text(&search.stdout), "");
    let stderr = text(&search.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("error: the index has no vectors"),
        "{stderr}"
    );

    let missing = annatar(dir.path(), &["show", "com.acme.sample.Missing"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(text(&missing.stdout), "");
    assert_eq!(
        text(&missing.stderr),
        "error: symbol `com.acme.sample.Missing` not found in the index\n"
    );
}

#[test]
fn search_without_an_index_is_one_error_line() {
    let dir = workspace();

    let search = annatar(
        dir.path(),
        &["search", "--kind", "method", "-k", "3", "users"],
    );

    assert_eq!(search.status.code(), Some(1));
    assert_eq!(text(&search.stdout), "");
    let stderr = text(&search.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("error: index database ")
            && stderr.contains("run `annatar index` first"),
        "{stderr}"
    );
    assert!(
        !dir.path().join("data").exists(),
        "search created the data dir"
    );
}

#[test]
fn search_rejects_unknown_filters_and_limits() {
    let dir = workspace();
    for args in [
        &["search", "--kind", "field", "users"][..],
        &["search", "--role", "dao", "users"][..],
        &["search", "-k", "0", "users"][..],
        &["search", "-k", "101", "users"][..],
    ] {
        let output = annatar(dir.path(), args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&output.stdout), "");
    }
}

/// Every file in `dir` with its contents, sorted by name.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

#[tokio::test]
async fn search_leaves_the_data_dir_unchanged() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    let data = dir.path().join("data");
    std::fs::remove_file(data.join(annatar::store::CACHE_DB)).unwrap();
    {
        let db = libsql::Builder::new_local(data.join(annatar::store::INDEX_DB))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        annatar::schema::create_vectors(&conn, 3).await.unwrap();
        conn.execute(
            "INSERT INTO symbol_vectors (symbol_id, embedding) SELECT id, vector32('[1, 0, 0]') FROM symbols",
            (),
        )
        .await
        .unwrap();
        for (key, value) in [
            (annatar::schema::META_EMBEDDING_MODEL, "other"),
            (annatar::schema::META_EMBEDDING_DIM, "3"),
        ] {
            conn.execute(
                "INSERT INTO index_meta (key, value) VALUES (?1, ?2)",
                libsql::params![key, value],
            )
            .await
            .unwrap();
        }
    }
    let before = files(&data);
    assert_eq!(
        before
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [annatar::store::INDEX_DB]
    );

    for args in [
        &["search", "users"][..],
        &["--path", ".", "search", "--kind", "method", "users"][..],
    ] {
        let search = annatar(dir.path(), args);
        assert_eq!(search.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&search.stdout), "");
        let stderr = text(&search.stderr);
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(
            stderr.starts_with("error: the index was embedded with \"other\""),
            "{stderr}"
        );
        assert!(files(&data) == before, "search changed the data dir");
    }
}

#[test]
fn search_path_outside_the_repo_is_one_error_line() {
    let dir = workspace();
    for prefix in ["..", "../repo", "src/../../x"] {
        let search = annatar(dir.path(), &["--path", prefix, "search", "users"]);
        assert_eq!(search.status.code(), Some(1), "{prefix}");
        assert_eq!(text(&search.stdout), "");
        assert_eq!(
            text(&search.stderr),
            format!("error: --path {prefix} is outside the repository ./repo\n")
        );
    }
}

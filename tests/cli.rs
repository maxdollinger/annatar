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

/// Run the binary in `dir` with no proxy, so requests to the fake servers on
/// loopback never leave the machine whatever the caller's environment says.
fn annatar(dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_annatar"));
    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(var);
    }
    command
        .current_dir(dir)
        .env_remove("RUST_LOG")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
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
fn show_lists_usages_capped_by_k() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));

    let show = annatar(
        dir.path(),
        &["show", "-k", "1", "com.acme.sample.UserService"],
    );
    let all = annatar(dir.path(), &["show", "com.acme.sample.UserService"]);
    let zero = annatar(
        dir.path(),
        &["show", "-k", "0", "com.acme.sample.UserService"],
    );

    assert!(show.status.success(), "{}", text(&show.stderr));
    assert!(
        text(&show.stdout).contains(
            "  - used by:\n    src/main/java/com/acme/sample/UserController.java\n      com.acme.sample.UserController reference :16\n    … 3 more\n  - uses (lines in this symbol's file):\n    src/main/java/com/acme/sample/User.java\n      com.acme.sample.User reference :12, 16, 20\n"
        ),
        "{}",
        text(&show.stdout)
    );
    assert!(
        text(&all.stdout).contains(
            "      com.acme.sample.UserController#list() :27 [entry: @GetMapping]\n      com.acme.sample.UserController#get(Long) :32 [entry: @GetMapping]\n  - uses (lines in this symbol's file):"
        ),
        "{}",
        text(&all.stdout)
    );
    assert_eq!(zero.status.code(), Some(2));
    assert!(
        text(&zero.stderr).contains("expected a number from 1 to 100"),
        "{}",
        text(&zero.stderr)
    );
}

#[test]
fn trace_prints_callers_and_rejects_depths_and_limits_out_of_range() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));

    let trace = annatar(
        dir.path(),
        &["trace", "com.acme.sample.UserService#find(Long)"],
    );
    assert!(trace.status.success(), "{}", text(&trace.stderr));
    assert_eq!(
        text(&trace.stdout),
        "com.acme.sample.UserService#find(Long) [method] src/main/java/com/acme/sample/UserService.java:16-18\n  com.acme.sample.UserController#get(Long) src/main/java/com/acme/sample/UserController.java:32 [entry: @GetMapping]\n"
    );
    let capped = annatar(
        dir.path(),
        &[
            "trace",
            "--depth",
            "1",
            "-k",
            "1",
            "com.acme.sample.UserService",
        ],
    );
    assert!(capped.status.success(), "{}", text(&capped.stderr));
    assert!(
        text(&capped.stdout).ends_with("  … 3 more\n"),
        "{}",
        text(&capped.stdout)
    );

    for args in [
        ["--depth", "0"],
        ["--depth", "11"],
        ["-k", "0"],
        ["-k", "101"],
    ] {
        let rejected = annatar(
            dir.path(),
            &["trace", args[0], args[1], "com.acme.sample.UserService"],
        );
        assert_eq!(rejected.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&rejected.stdout), "");
        let expected = if args[0] == "--depth" {
            "expected a number from 1 to 10"
        } else {
            "expected a number from 1 to 100"
        };
        assert!(
            text(&rejected.stderr).contains(expected),
            "{}",
            text(&rejected.stderr)
        );
    }

    let missing = annatar(dir.path(), &["trace", "com.acme.sample.Missing"]);
    assert_eq!(missing.status.code(), Some(1));
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
        &["search", "--symbols", "-k", "101", "users"][..],
    ] {
        let output = annatar(dir.path(), args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&output.stdout), "");
        if args.contains(&"-k") {
            assert!(
                text(&output.stderr).contains("expected files: 1 to 20; with --symbols: 1 to 100"),
                "{}",
                text(&output.stderr)
            );
        }
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

/// An embedding server on localhost that answers every request with the
/// vector `[1, 0, 0]` and counts the requests.
fn embedding_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::atomic::Ordering;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = served.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = r#"{"data": [{"embedding": [1, 0, 0], "index": 0}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
    });
    (url, served)
}

/// Point `dir`'s config at a fresh [`embedding_server`] and give every
/// symbol n of its index the vector [1, n, 0], so against the server's
/// query vector [1, 0, 0] the symbols rank in id order. Returns the
/// server's request counter and every symbol's fqn and file in id order.
async fn with_vectors(
    dir: &Path,
) -> (
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    Vec<(String, String)>,
) {
    let (url, served) = embedding_server();
    std::fs::write(
        dir.join("annatar.toml"),
        format!(
            "repo = \"repo\"\ndata_dir = \"data\"\n\n[ollama]\nurl = \"{url}\"\nchat_model = \"chat\"\nembedding_model = \"embed\"\n"
        ),
    )
    .unwrap();
    let db = libsql::Builder::new_local(dir.join("data").join(annatar::store::INDEX_DB))
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    annatar::schema::create_vectors(&conn, 3).await.unwrap();
    conn.execute(
        "INSERT INTO symbol_vectors (symbol_id, embedding) SELECT id, vector32('[1, ' || id || ', 0]') FROM symbols",
        (),
    )
    .await
    .unwrap();
    for (key, value) in [
        (annatar::schema::META_EMBEDDING_MODEL, "embed"),
        (annatar::schema::META_EMBEDDING_DIM, "3"),
        (annatar::schema::META_EMBEDDING_PARENT_DESCRIPTION, "false"),
    ] {
        conn.execute(
            "INSERT INTO index_meta (key, value) VALUES (?1, ?2)",
            libsql::params![key, value],
        )
        .await
        .unwrap();
    }
    let mut rows = conn
        .query("SELECT fqn, file FROM symbols ORDER BY id", ())
        .await
        .unwrap();
    let mut order = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        order.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
    }
    (served, order)
}

#[tokio::test]
async fn eval_scores_a_golden_set_through_search() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    let (served, order) = with_vectors(dir.path()).await;
    let mut files: Vec<&str> = Vec::new();
    for (_, file) in &order {
        if !files.contains(&file.as_str()) {
            files.push(file);
        }
    }
    let golden =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden/sample.toml");
    let set = annatar::golden::GoldenSet::load(&golden).unwrap();

    let eval = annatar(dir.path(), &["eval", "-k", "20", golden.to_str().unwrap()]);

    assert!(eval.status.success(), "{}", text(&eval.stderr));
    let rank = |fqn: &str| order.iter().position(|(other, _)| other == fqn).unwrap() + 1;
    let file_rank = |fqn: &str| {
        let file = &order.iter().find(|(other, _)| other == fqn).unwrap().1;
        files.iter().position(|other| other == file).unwrap() + 1
    };
    let mut expected =
        "eval: embedding_model=embed dim=3 parent_description=false k=20\n".to_string();
    for (number, question) in set.questions().iter().enumerate() {
        let primary = rank(question.primary());
        let any = question.expect().iter().map(|fqn| rank(fqn)).min().unwrap();
        let primary_file = file_rank(question.primary());
        let any_file = question
            .expect()
            .iter()
            .map(|fqn| file_rank(fqn))
            .min()
            .unwrap();
        let group = if question.primary().contains('#') {
            "members"
        } else {
            "types"
        };
        expected.push_str(&format!(
            "{}. {primary} {any} file {primary_file} {any_file} [{group}] {}",
            number + 1,
            question.primary()
        ));
        if primary != 1 {
            expected.push_str(&format!(" top={}", order[0].0));
        }
        expected.push('\n');
    }
    let stdout = text(&eval.stdout);
    assert!(
        stdout.starts_with(&expected),
        "{stdout}\nexpected:\n{expected}"
    );
    let summaries: Vec<&str> = stdout.lines().skip(set.questions().len() + 1).collect();
    assert_eq!(summaries.len(), 6, "{stdout}");
    assert!(
        summaries[0].starts_with("all 5: primary top-1 "),
        "{stdout}"
    );
    assert!(summaries[1].starts_with("types 2: "), "{stdout}");
    assert!(summaries[2].starts_with("members 3: "), "{stdout}");
    assert!(
        summaries[3].starts_with("files all 5: primary top-1 "),
        "{stdout}"
    );
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 5);
}

#[test]
fn eval_fails_on_a_golden_symbol_missing_from_the_index() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    std::fs::write(
        dir.path().join("golden.toml"),
        "[[question]]\ntext = \"Who lists users?\"\nexpect = [\"com.acme.sample.UserController#list()\", \"com.acme.sample.Gone\"]\n",
    )
    .unwrap();

    let eval = annatar(dir.path(), &["eval", "golden.toml"]);

    assert_eq!(eval.status.code(), Some(1));
    assert_eq!(text(&eval.stdout), "");
    assert_eq!(
        text(&eval.stderr),
        "error: the golden set does not match the index: com.acme.sample.Gone is not in the index (\"Who lists users?\")\n"
    );
}

#[test]
fn eval_rejects_limits_below_five() {
    let dir = workspace();
    for args in [
        &["eval", "-k", "4", "golden.toml"][..],
        &["eval", "-k", "101", "golden.toml"][..],
    ] {
        let output = annatar(dir.path(), args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&output.stdout), "");
        assert!(
            text(&output.stderr).contains("expected a number from 5 to 100"),
            "{}",
            text(&output.stderr)
        );
    }
}

#[test]
fn eval_takes_no_path() {
    let dir = workspace();

    let eval = annatar(dir.path(), &["--path", "src", "eval", "golden.toml"]);

    assert_eq!(eval.status.code(), Some(1));
    assert_eq!(
        text(&eval.stderr),
        "error: eval scores the whole index and takes no --path\n"
    );
}

#[test]
fn eval_usages_scores_the_edges_without_ollama() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    let set = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden/usages.toml");

    let eval = annatar(dir.path(), &["eval-usages", set.to_str().unwrap()]);

    assert!(eval.status.success(), "{}", text(&eval.stderr));
    let stdout = text(&eval.stdout);
    assert!(
        stdout
            .starts_with("1. P 1.000 R 1.000 (4/4, 4 found) [types] com.acme.sample.UserService\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with(
            "all 4: P 1.000 R 1.000 (12/12, 12 found)\n\
             types 2: P 1.000 R 1.000 (11/11, 11 found)\n\
             members 2: P 1.000 R 1.000 (1/1, 1 found)\n\
             overridden by 1: P - R - (0/0, 0 found)\n"
        ),
        "{stdout}"
    );
}

#[test]
fn eval_usages_fails_on_a_renamed_symbol_and_takes_no_path() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    std::fs::write(
        dir.path().join("usages.toml"),
        "[[symbol]]\nfqn = \"com.acme.sample.UserService#lookup(Long)\"\nusers = []\n",
    )
    .unwrap();

    let eval = annatar(dir.path(), &["eval-usages", "usages.toml"]);
    let with_path = annatar(dir.path(), &["--path", "src", "eval-usages", "usages.toml"]);

    assert_eq!(eval.status.code(), Some(1));
    assert_eq!(text(&eval.stdout), "");
    assert_eq!(
        text(&eval.stderr),
        "error: the usage golden set does not match the index: com.acme.sample.UserService#lookup(Long) is not in the index (\"symbol 1\")\n"
    );
    assert_eq!(with_path.status.code(), Some(1));
    assert_eq!(
        text(&with_path.stderr),
        "error: eval-usages scores the whole index and takes no --path\n"
    );
}

#[tokio::test]
async fn search_prints_files_by_default_and_symbols_on_request() {
    let dir = workspace();
    let index = annatar(dir.path(), &["index", "--no-llm"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    let (served, _) = with_vectors(dir.path()).await;

    let files = annatar(dir.path(), &["search", "-k", "2", "users"]);
    let symbols = annatar(dir.path(), &["search", "--symbols", "-k", "3", "users"]);

    assert!(files.status.success(), "{}", text(&files.stderr));
    assert_eq!(
        text(&files.stdout),
        "\
1. 0.707 src/main/java/com/acme/sample/User.java
   class com.acme.sample.User [entity] :10-25 *0.707
     #getId() :18-20 *0.447
     #getName() :22-24 *0.316
2. 0.243 src/main/java/com/acme/sample/UserController.java
   class com.acme.sample.UserController [controller] :13-41 *0.243
     #<init>(UserService) :18-20 *0.196
     #list() :25-28 *0.164
     #get(Long) :30-33 *0.141
     class ErrorResponse :38-40 *0.124
"
    );
    assert!(symbols.status.success(), "{}", text(&symbols.stderr));
    assert_eq!(
        text(&symbols.stdout),
        "\
1. 0.707 com.acme.sample.User [class] role=entity src/main/java/com/acme/sample/User.java:10-25
2. 0.447 com.acme.sample.User#getId() [method] src/main/java/com/acme/sample/User.java:18-20
3. 0.316 com.acme.sample.User#getName() [method] src/main/java/com/acme/sample/User.java:22-24
"
    );

    let many = annatar(dir.path(), &["search", "-k", "21", "users"]);
    assert_eq!(many.status.code(), Some(2));
    assert_eq!(text(&many.stdout), "");
    let stderr = text(&many.stderr);
    assert!(
        stderr.contains(
            "-k 21: search prints at most 20 files; use --symbols for more (files: 1 to 20; with --symbols: 1 to 100)"
        ) && stderr.contains("Usage: annatar search "),
        "{stderr}"
    );
    let many = annatar(dir.path(), &["search", "--symbols", "-k", "21", "users"]);
    assert!(many.status.success(), "{}", text(&many.stderr));
    assert_eq!(text(&many.stdout).lines().count(), 14);
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 3);
}

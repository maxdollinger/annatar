//! Reproducible synthetic benchmark for step 2.3.
//!
//! Generates a scratch git repository of [`FILES`] Java classes, each with
//! [`METHODS`] methods (so `FILES * (METHODS + 1)` symbols), over a run of
//! [`COMMITS`] ticket-tagged commits that touch the classes over time. It then
//! times three index runs against one `data_dir`:
//!
//! 1. **cold** — a fresh cache, so every symbol shells out to `git log -L`.
//! 2. **warm** — unchanged sources, so every symbol should hit the history
//!    cache and skip git.
//! 3. **changed** — about a tenth of the files rewritten in one new commit, so
//!    only those files' symbols miss.
//!
//! No network and no `--release`: the timings are from a debug build and
//! overstate release run time. They are printed, never asserted; only the
//! exact hit/miss counts are. Run it with:
//!
//! ```text
//! cargo test --test benchmark -- --ignored --nocapture
//! ```

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use annatar::config::DEFAULT_TICKET_REGEX;
use annatar::indexer::{self, IndexStats};
use annatar::store::Store;
use regex::Regex;

#[path = "../src/test_support.rs"]
mod test_support;

use test_support::{commit, init_repo};

/// Java classes generated in the scratch repo.
const FILES: usize = 200;
/// Methods per class, so each file yields one type plus this many members.
const METHODS: usize = 4;
/// Ticket-tagged commits that rewrite files over time.
const COMMITS: usize = 20;
/// Fraction of files rewritten for the third run (10%).
const CHANGED_DIVISOR: usize = 10;

/// A deterministic ISO 8601 date, one minute apart per commit.
fn date(minute: usize) -> String {
    format!("2024-01-01T00:{minute:02}:00+00:00")
}

fn class_source(class: usize, version: usize) -> String {
    let mut out = format!("package com.bench;\n\npublic class C{class} {{\n");
    for method in 0..METHODS {
        out.push_str(&format!(
            "    /** method {method} */\n    public int m{method}() {{\n        return {};\n    }}\n",
            version * 100 + method
        ));
    }
    out.push_str("}\n");
    out
}

fn write_class(repo: &Path, class: usize, version: usize) {
    let dir = repo.join("src/main/java/com/bench");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("C{class}.java")),
        class_source(class, version),
    )
    .unwrap();
}

/// Create the scratch repo and its commit history.
fn generate(repo: &Path) {
    init_repo(repo);
    for class in 0..FILES {
        write_class(repo, class, 0);
    }
    commit(repo, "GRLD-1 initial import", &date(0));
    for c in 1..=COMMITS {
        for class in 0..FILES {
            if (class + c) % 3 == 0 {
                write_class(repo, class, c);
            }
        }
        commit(repo, &format!("GRLD-{} commit {c}", c + 1), &date(c));
    }
}

async fn time_run(repo: &Path, data: &Path, regex: &Regex) -> (IndexStats, Duration) {
    let store = Store::open(data).await.unwrap();
    let start = Instant::now();
    let stats = indexer::build_index(&store, repo, None, regex, None)
        .await
        .unwrap();
    (stats, start.elapsed())
}

fn machine_summary() -> String {
    let cpus = if cfg!(target_os = "macos") {
        Command::new("sysctl")
            .args(["-n", "hw.ncpu"])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_else(|| "?".to_string())
    } else {
        "?".to_string()
    };
    format!("os={} cpus={cpus}", std::env::consts::OS)
}

#[tokio::test]
#[ignore = "benchmark; run with --ignored --nocapture"]
async fn synthetic_index_benchmark() {
    let repo_dir = tempfile::tempdir().unwrap();
    let repo = repo_dir.path();
    let generated = Instant::now();
    generate(repo);
    let generated = generated.elapsed();

    let data = tempfile::tempdir().unwrap();
    let regex = Regex::new(DEFAULT_TICKET_REGEX).unwrap();

    let (cold, cold_time) = time_run(repo, data.path(), &regex).await;
    let (warm, warm_time) = time_run(repo, data.path(), &regex).await;

    let changed_files = FILES / CHANGED_DIVISOR;
    for class in 0..changed_files {
        write_class(repo, class, 1000 + class);
    }
    commit(repo, "GRLD-99 rewrite a tenth", &date(COMMITS + 1));
    let (changed, changed_time) = time_run(repo, data.path(), &regex).await;

    println!("\n=== Annatar index benchmark ===");
    println!("machine: {}", machine_summary());
    println!(
        "generated: {} files, {} methods/file, {} commits in {generated:?}",
        FILES, METHODS, COMMITS
    );
    println!();
    println!(
        "{:<10} {:>8} {:>10} {:>10} {:>10} {:>10} {:>9}",
        "run", "files", "symbols", "commits", "tickets", "hits", "misses"
    );
    for (name, stats, time) in [
        ("cold", cold, cold_time),
        ("warm", warm, warm_time),
        ("changed", changed, changed_time),
    ] {
        println!(
            "{name:<10} {:>8} {:>10} {:>10} {:>10} {:>10} {:>9}",
            stats.files,
            stats.symbols,
            stats.commits,
            stats.tickets,
            stats.history_hits,
            stats.history_misses
        );
        println!("{name} wall time: {time:?}");
    }

    let symbols = FILES * (METHODS + 1);
    assert_eq!(cold.symbols, symbols, "every generated symbol is indexed");
    for stats in [cold, warm, changed] {
        assert_eq!(stats.history_skipped, 0, "every file is committed");
    }
    assert_eq!(
        (cold.history_hits, cold.history_misses),
        (0, symbols),
        "a cold run misses every symbol"
    );
    assert_eq!(
        (warm.history_hits, warm.history_misses),
        (symbols, 0),
        "an unchanged tree hits every symbol"
    );
    let changed_symbols = changed_files * (METHODS + 1);
    assert_eq!(
        (changed.history_hits, changed.history_misses),
        (symbols - changed_symbols, changed_symbols),
        "exactly the rewritten files miss"
    );
}

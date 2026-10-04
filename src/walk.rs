//! The production Java files to index.
//!
//! A single pure function, [`java_files`], walks a repository with the
//! [`ignore`] crate so `.gitignore` and the other standard filters apply, then
//! drops test code and build/generated trees. Results are paths relative to
//! the repository root, sorted so later steps and tests are stable.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use ignore::{DirEntry, WalkBuilder};

/// Directory names that are build output or generated source trees: `build`
/// and `target` are Maven/Gradle output, `generated` and `generated-sources`
/// are build-time source trees. A directory carrying one of these names is
/// pruned only outside a source root, so a legitimate package such as
/// `com.acme.build` under `src/main/java` is still indexed.
const SKIP_DIRS: &[&str] = &["build", "target", "generated", "generated-sources"];

/// Every production `.java` file under `repo`, as paths relative to `repo`,
/// sorted lexicographically.
///
/// `.gitignore` is honoured (the `ignore` crate's standard filters), and the
/// [`SKIP_DIRS`] trees and test source sets (see [`is_test`]) are excluded. `path_prefix`, when given,
/// limits the result to files under that prefix; it may be relative to `repo`
/// or an absolute path inside it. A prefix outside the repository, or one that
/// matches nothing, yields an empty list rather than an error.
pub fn java_files(repo: &Path, path_prefix: Option<&Path>) -> Result<Vec<PathBuf>> {
    let prefix = match path_prefix {
        Some(prefix) => match relative_prefix(repo, prefix) {
            Some(prefix) => Some(prefix),
            None => return Ok(Vec::new()),
        },
        None => None,
    };

    let root = repo.to_path_buf();
    let walk_prefix = prefix.clone();
    let mut walker = WalkBuilder::new(repo);
    walker
        .standard_filters(true)
        .parents(false)
        .require_git(false)
        .filter_entry(move |entry| {
            !is_skipped_dir(&root, entry) && descend(&root, walk_prefix.as_deref(), entry)
        });

    let mut files = Vec::new();
    for result in walker.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!("skipping unreadable entry: {err}");
                continue;
            }
        };
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(repo) else {
            continue;
        };
        if !is_java(relative) {
            continue;
        }
        if is_test(relative) {
            tracing::debug!(path = %relative.display(), "skipping test file");
            continue;
        }
        if let Some(prefix) = &prefix
            && !relative.starts_with(prefix)
        {
            continue;
        }
        files.push(relative.to_path_buf());
    }
    files.sort();
    Ok(files)
}

fn is_java(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "java")
}

/// Whether `path` lies in a test source set.
///
/// The source set is the `<set>` of the first `src/<set>/java` directory
/// triple (the source root), at the repository root or in a module
/// (`backend/src/test/java`); without one, the first `src/<set>` pair
/// (`src/test/kotlin`, `src/test/resources`). Only that set counts, so a
/// package `com.acme.src.test` under `src/main/java` stays production code,
/// and a module directory named `java` (`java/src/test/java`) does not hide
/// the set. See [`is_test_set`] for the names.
fn is_test(path: &Path) -> bool {
    let dirs: Vec<_> = path
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .map(|component| component.as_os_str())
        .collect();
    dirs.windows(3)
        .find(|triple| triple[0] == "src" && triple[2] == "java")
        .or_else(|| dirs.windows(2).find(|pair| pair[0] == "src"))
        .is_some_and(|window| is_test_set(&window[1].to_string_lossy()))
}

/// Whether a source set named `set` holds tests: `test`, or `test` as the
/// first or last word of a camelCase, kebab or snake name (Gradle's
/// `testFixtures`, `integrationTest`, `androidTest`; `test-fixtures`,
/// `integration_test`). `testing`, `testng` and `contest` are not.
fn is_test_set(set: &str) -> bool {
    let starts = set
        .strip_prefix("test")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|next| !next.is_ascii_lowercase());
    let ends = ["Test", "-test", "_test"]
        .iter()
        .any(|suffix| set.len() > suffix.len() && set.ends_with(suffix));
    set == "test" || starts || ends
}

/// Whether `entry` is a directory to prune from the walk.
///
/// A directory whose name is in [`SKIP_DIRS`] is build output or a generated
/// source tree and is pruned, unless it sits under a source root (an ancestor
/// component named `java`), where the same name may be a legitimate package.
fn is_skipped_dir(repo: &Path, entry: &DirEntry) -> bool {
    if !entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
    {
        return false;
    }
    let Ok(relative) = entry.path().strip_prefix(repo) else {
        return false;
    };
    let mut components = relative.components();
    let Some(name) = components.next_back() else {
        return false;
    };
    let name = name.as_os_str().to_string_lossy();
    if !SKIP_DIRS.iter().any(|skip| name == *skip) {
        return false;
    }
    !components.any(|component| component.as_os_str() == "java")
}

/// Whether the walk should descend into `entry` under a `--path` prefix.
///
/// Files are always kept (the caller filters them); a directory only when
/// [`on_prefix_path`] says so.
fn descend(repo: &Path, prefix: Option<&Path>, entry: &DirEntry) -> bool {
    let Some(prefix) = prefix else {
        return true;
    };
    if !entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
    {
        return true;
    }
    entry
        .path()
        .strip_prefix(repo)
        .map_or(true, |relative| on_prefix_path(relative, prefix))
}

/// Whether the repo-relative directory `relative` is the prefix, inside it, or
/// one of its ancestors. Everything else is pruned, so the rest of the
/// repository is never visited. Ancestors are still walked rather than starting
/// at `repo/prefix`, because that is how the walker reads their `.gitignore`.
fn on_prefix_path(relative: &Path, prefix: &Path) -> bool {
    relative.starts_with(prefix) || prefix.starts_with(relative)
}

/// Reduce `prefix` to a path relative to `repo`. Absolute prefixes that fall
/// outside `repo` return `None`; a relative prefix is normalised by dropping
/// leading `./` components.
pub fn relative_prefix(repo: &Path, prefix: &Path) -> Option<PathBuf> {
    if prefix.is_absolute() {
        if let Ok(relative) = prefix.strip_prefix(repo) {
            return Some(relative.to_path_buf());
        }
        if repo.is_relative()
            && let Ok(cwd) = std::env::current_dir()
            && let Ok(relative) = prefix.strip_prefix(cwd.join(repo))
        {
            return Some(relative.to_path_buf());
        }
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in prefix.components() {
        if let Component::CurDir = component {
            continue;
        }
        normalized.push(component.as_os_str());
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(repo: &Path, relative: &str) {
        let path = repo.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "class Placeholder {}\n").unwrap();
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn returns_exactly_the_expected_production_files() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        for file in [
            "src/main/java/com/acme/App.java",
            "src/main/java/com/acme/Util.java",
            "src/main/java/com/acme/package-info.txt",
            "src/test/java/com/acme/AppTest.java",
            "generated/Generated.java",
            "build/Out.java",
            "target/Target.java",
            "generated-sources/Source.java",
            "vendor/Vendor.java",
        ] {
            write_file(root, file);
        }
        std::fs::write(root.join(".gitignore"), "vendor/\n").unwrap();

        let files = java_files(root, None).unwrap();

        assert_eq!(
            files,
            paths(&[
                "src/main/java/com/acme/App.java",
                "src/main/java/com/acme/Util.java",
            ]),
            "walker should return only the two production files"
        );
    }

    #[test]
    fn gitignore_excludes_ignored_files() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/Keep.java");
        write_file(root, "src/main/java/com/acme/Drop.java");
        std::fs::write(root.join(".gitignore"), "Drop.java\n").unwrap();

        let files = java_files(root, None).unwrap();

        assert_eq!(files, paths(&["src/main/java/com/acme/Keep.java"]));
    }

    #[test]
    fn skips_build_output_and_generated_trees() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/Real.java");
        for skip in SKIP_DIRS {
            write_file(root, &format!("{skip}/Ignored.java"));
            write_file(root, &format!("module/{skip}/Nested/Ignored.java"));
        }

        let files = java_files(root, None).unwrap();

        assert_eq!(files, paths(&["src/main/java/com/acme/Real.java"]));
    }

    #[test]
    fn keeps_package_dirs_named_build_under_a_source_root() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/build/Keep.java");
        write_file(root, "build/Out.java");

        let files = java_files(root, None).unwrap();

        assert_eq!(
            files,
            paths(&["src/main/java/com/acme/build/Keep.java"]),
            "a package named build under a source root is indexed; build output is pruned"
        );
    }

    #[test]
    fn skips_src_test_tree() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/App.java");
        write_file(root, "src/test/java/com/acme/AppTest.java");
        write_file(root, "src/test/kotlin/com/acme/AppSpec.java");
        write_file(root, "src/testng/com/acme/NotTest.java");

        let files = java_files(root, None).unwrap();

        assert_eq!(
            files,
            paths(&[
                "src/main/java/com/acme/App.java",
                "src/testng/com/acme/NotTest.java",
            ]),
            "only the src/test prefix is test code"
        );
    }

    #[test]
    fn skips_module_src_test_trees_but_not_packages_named_test() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "backend/src/main/java/com/acme/App.java");
        write_file(root, "backend/src/test/java/com/acme/AppTest.java");
        write_file(root, "a/b/src/test/java/com/acme/DeepTest.java");
        write_file(root, "src/main/java/com/acme/src/test/Keep.java");
        write_file(root, "backend/src/testng/com/acme/NotTest.java");
        write_file(root, "backend/test/src/com/acme/AlsoKept.java");

        let files = java_files(root, None).unwrap();

        assert_eq!(
            files,
            paths(&[
                "backend/src/main/java/com/acme/App.java",
                "backend/src/testng/com/acme/NotTest.java",
                "backend/test/src/com/acme/AlsoKept.java",
                "src/main/java/com/acme/src/test/Keep.java",
            ]),
            "a module's src/test is test code; src/test under a source root is a package"
        );
    }

    #[test]
    fn path_prefix_limits_results_and_accepts_absolute_prefixes() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/App.java");
        write_file(root, "src/main/java/com/acme/other/Other.java");

        let relative =
            java_files(root, Some(Path::new("src/main/java/com/acme/App.java"))).unwrap();
        assert_eq!(relative, paths(&["src/main/java/com/acme/App.java"]));

        let absolute = java_files(root, Some(&root.join("src/main/java/com/acme/other"))).unwrap();
        assert_eq!(
            absolute,
            paths(&["src/main/java/com/acme/other/Other.java"])
        );

        let dot = java_files(root, Some(Path::new("./src/main/java/com/acme"))).unwrap();
        assert_eq!(
            dot,
            paths(&[
                "src/main/java/com/acme/App.java",
                "src/main/java/com/acme/other/Other.java",
            ])
        );
    }

    #[test]
    fn path_prefix_keeps_ancestor_gitignores() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/App.java");
        write_file(root, "src/main/java/com/acme/scratch/Ignored.java");
        write_file(root, "src/main/java/com/other/Other.java");
        std::fs::write(root.join(".gitignore"), "scratch/\n").unwrap();

        assert_eq!(
            java_files(root, Some(Path::new("src/main/java/com/acme"))).unwrap(),
            paths(&["src/main/java/com/acme/App.java"]),
            "the root .gitignore still applies under the prefix"
        );
    }

    #[test]
    fn path_prefix_prunes_directories_off_its_path() {
        let prefix = Path::new("src/main/java/com/acme");
        for kept in [
            "",
            "src",
            "src/main/java/com",
            "src/main/java/com/acme",
            "src/main/java/com/acme/user",
        ] {
            assert!(on_prefix_path(Path::new(kept), prefix), "{kept:?} is kept");
        }
        for pruned in [
            "docs",
            "src/main/resources",
            "src/main/java/com/other",
            "src/main/java/com/acmexyz",
        ] {
            assert!(
                !on_prefix_path(Path::new(pruned), prefix),
                "{pruned:?} is pruned"
            );
        }
    }

    #[test]
    fn path_prefix_outside_repo_or_matching_nothing_is_empty() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "src/main/java/com/acme/App.java");

        assert!(
            java_files(root, Some(Path::new("/does/not/exist")))
                .unwrap()
                .is_empty(),
            "an absolute prefix outside the repo should be empty"
        );
        assert!(
            java_files(root, Some(Path::new("src/main/java/com/missing")))
                .unwrap()
                .is_empty(),
            "a prefix matching nothing should be empty"
        );
    }

    #[test]
    fn test_source_sets_are_found_by_their_source_root() {
        for test in [
            "src/test/java/A.java",
            "java/src/test/java/A.java",
            "backend/src/test/resources/A.java",
            "src/test/kotlin/A.java",
            "src/integrationTest/java/A.java",
            "src/testFixtures/java/A.java",
            "app/src/androidTest/java/A.java",
            "src/test-fixtures/java/A.java",
            "src/integration_test/java/A.java",
            "src/backend/src/test/java/A.java",
        ] {
            assert!(is_test(Path::new(test)), "{test} is test code");
        }
        for production in [
            "src/main/java/A.java",
            "java/src/main/java/A.java",
            "src/main/java/com/acme/src/test/Foo.java",
            "src/main/java/com/acme/test/Foo.java",
            "src/main/java/com/acme/src/integrationTest/java/Foo.java",
            "src/main/kotlin/com/acme/src/test/Foo.java",
            "src/testing/java/A.java",
            "src/testng/java/A.java",
            "src/contest/java/A.java",
            "src/main/javatest/src/test/A.java",
            "test/src/main/java/A.java",
            "src/test.java",
            "A.java",
        ] {
            assert!(
                !is_test(Path::new(production)),
                "{production} is production"
            );
        }
    }

    #[test]
    fn path_prefix_and_test_sets_combine() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        write_file(root, "java/src/main/java/com/acme/App.java");
        write_file(root, "java/src/test/java/com/acme/AppTest.java");
        write_file(root, "java/src/integrationTest/java/com/acme/AppIT.java");
        write_file(root, "java/src/main/java/com/acme/src/test/Keep.java");

        assert_eq!(
            java_files(root, None).unwrap(),
            paths(&[
                "java/src/main/java/com/acme/App.java",
                "java/src/main/java/com/acme/src/test/Keep.java",
            ])
        );
        assert!(
            java_files(root, Some(Path::new("java/src/test")))
                .unwrap()
                .is_empty(),
            "a prefix inside a test set yields nothing"
        );
        assert_eq!(
            java_files(root, Some(Path::new("java/src/main/java/com/acme/src"))).unwrap(),
            paths(&["java/src/main/java/com/acme/src/test/Keep.java"]),
            "a prefix down to a package named src/test keeps it"
        );
    }
}

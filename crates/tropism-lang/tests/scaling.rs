//! Guards against reintroducing D39: O(project) work per import.
//!
//! Five providers shipped this bug independently — Rust rebuilt its module set per
//! import, JavaScript walked every source file per relative import, Python, Java and
//! C# walked every module inside `longest_prefix`, and C++ scanned every module for
//! a path suffix. Each was quadratic over a project and invisible below about a
//! thousand files. Each was found by benchmarking rather than by review, and nothing
//! stopped the next one.
//!
//! **What this measures, and why it is not a benchmark.** Resolving one import must
//! cost the same whether the project has 500 modules or 2,000. So a *fixed* number
//! of imports is resolved against contexts of two very different sizes, and the two
//! totals are compared. Correct code is flat; a linear scan is 4x.
//!
//! That framing is what makes the test survive CI:
//!
//! * Nothing touches the disk and nothing is parsed. The measurement is the
//!   resolution loop and only the resolution loop, so the signal is not buried under
//!   I/O or tree-sitter.
//! * The expected separation is 1x against 4x, and the threshold sits between them
//!   with room on both sides.
//! * Each size is timed five times and the best run is used, because a scheduler
//!   preemption can only ever make a run look *slower*.
//!
//! Every one of these tests was verified to *fail* by reintroducing the bug it
//! guards — a regression test that does not fail on the regression is decoration.
//!
//! See D39 in `design/12-known-limitations.md`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use tropism_core::model::{Language, Project};
use tropism_core::provider::{Import, LanguageProvider, ProjectContext};

/// Project sizes to compare. The ratio between them is what the assertion is about.
const SMALL: usize = 500;
const LARGE: usize = 2_000;

/// Imports resolved per measurement, the same at both sizes.
const RESOLUTIONS: usize = 2_000;

/// Growth allowed when the project grows 4x.
///
/// Correct resolution is `O(log n)` per import, so the expectation is ~1.0. A linear
/// scan is 4.0 by construction.
///
/// **Measured on this machine rather than reasoned about**, by reintroducing each
/// bug and re-running:
///
/// | | fixed | reintroduced |
/// | --- | --- | --- |
/// | debug   | 1.0x – 1.9x | 3.7x – 4.0x |
/// | release | 0.8x – 1.5x | — |
///
/// So 2.5 sits between the two populations with roughly 1.3x of margin below and
/// 1.5x above. That is narrower than it looks safe to make it: the gap cannot be
/// widened by raising the threshold without losing the ability to catch a scan over
/// a *fraction* of the project, and cannot be narrowed without risking the 1.9x
/// outlier, which is debug-build noise at ~2 ms absolute rather than real growth.
///
/// If this ever fails spuriously, raise [`RESOLUTIONS`] before raising this — more
/// work per measurement makes the ratio more stable, where a looser bound just makes
/// the test blinder.
const MAX_GROWTH: f64 = 2.5;

fn provider(language: Language) -> &'static dyn LanguageProvider {
    *tropism_lang::registry()
        .iter()
        .find(|candidate| candidate.language() == language)
        .expect("provider is compiled in")
}

fn project(language: Language) -> Project {
    Project {
        root: Utf8PathBuf::new(),
        language,
        manifests: vec![],
        lockfile: None,
    }
}

/// Times `RESOLUTIONS` imports against a context of `size`, best of three.
fn resolve_cost(
    language: Language,
    size: usize,
    modules: impl Fn(usize) -> String,
    files: impl Fn(usize) -> Utf8PathBuf,
    specifier: impl Fn(usize) -> String,
    from: &str,
) -> Duration {
    let provider = provider(language);
    let project = project(language);
    let known: BTreeSet<String> = (0..size).map(&modules).collect();
    let source_files: Vec<Utf8PathBuf> = (0..size).map(&files).collect();
    let from = Utf8PathBuf::from(from);

    // Precomputed, so string formatting is not inside the timed loop.
    let specifiers: Vec<Import> = (0..RESOLUTIONS)
        .map(|i| Import::statement(specifier(i % size), 1))
        .collect();

    let mut best = Duration::MAX;
    for _ in 0..5 {
        // A fresh context each round: `local_modules` memoizes per project, and
        // reusing it across rounds would hide the cost of building the index — which
        // is exactly the cost a future regression would move back into the loop.
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("app"),
            declared: &[],
            sibling_packages: &[],
            known_modules: &known,
            source_files: &source_files,
            local_modules: Default::default(),
            path_aliases: &[],
        };

        let start = Instant::now();
        for import in &specifiers {
            std::hint::black_box(provider.resolve_import(import, &from, &ctx));
        }
        best = best.min(start.elapsed());
    }
    best
}

/// Runs the comparison and fails with the numbers rather than a bare boolean.
fn assert_flat(
    label: &str,
    language: Language,
    modules: impl Fn(usize) -> String + Copy,
    files: impl Fn(usize) -> Utf8PathBuf + Copy,
    specifier: impl Fn(usize) -> String + Copy,
    from: &str,
) {
    let small = resolve_cost(language, SMALL, modules, files, specifier, from);
    let large = resolve_cost(language, LARGE, modules, files, specifier, from);

    let growth = large.as_secs_f64() / small.as_secs_f64().max(f64::MIN_POSITIVE);
    assert!(
        growth < MAX_GROWTH,
        "{label}: resolving {RESOLUTIONS} imports took {small:?} against {SMALL} modules \
         and {large:?} against {LARGE} — {growth:.1}x for a 4x project.\n\
         Per-import work is growing with project size, which is D39. Resolution must \
         be bounded by the import, not by the project: probe the input's own prefixes, \
         or index the set once via `ProjectContext::local_modules`.",
    );
}

#[test]
fn javascript_relative_resolution_does_not_scan_the_project() {
    assert_flat(
        "javascript",
        Language::JavaScript,
        |i| format!("src/m{i:05}"),
        |i| Utf8PathBuf::from(format!("src/m{i:05}.ts")),
        |i| format!("./m{i:05}"),
        "src/entry.ts",
    );
}

#[test]
fn python_resolution_does_not_scan_the_module_set() {
    assert_flat(
        "python",
        Language::Python,
        |i| format!("pkg.sub.m{i:05}"),
        |i| Utf8PathBuf::from(format!("pkg/sub/m{i:05}.py")),
        |i| format!("pkg.sub.m{i:05}"),
        "pkg/entry.py",
    );
}

#[test]
fn java_resolution_does_not_scan_the_module_set() {
    assert_flat(
        "java",
        Language::Java,
        |i| format!("com.app.sub.m{i:05}"),
        |i| Utf8PathBuf::from(format!("src/main/java/com/app/sub/M{i:05}.java")),
        |i| format!("com.app.sub.m{i:05}.Thing"),
        "src/main/java/com/app/Entry.java",
    );
}

#[test]
fn csharp_resolution_does_not_scan_the_module_set() {
    assert_flat(
        "csharp",
        Language::CSharp,
        |i| format!("App.Sub.M{i:05}"),
        |i| Utf8PathBuf::from(format!("App/Sub/M{i:05}.cs")),
        |i| format!("App.Sub.M{i:05}"),
        "App/Entry.cs",
    );
}

#[test]
fn cpp_resolution_does_not_scan_the_component_set() {
    assert_flat(
        "cpp",
        Language::Cpp,
        // Nested, so the include below matches on a suffix rather than exactly —
        // the path the C++ provider used to scan every component for.
        |i| format!("vendor/app/m{i:05}"),
        |i| Utf8PathBuf::from(format!("include/vendor/app/m{i:05}.hpp")),
        |i| format!("app/m{i:05}.hpp"),
        "src/entry.cpp",
    );
}

#[test]
fn rust_resolution_does_not_rebuild_the_module_set() {
    assert_flat(
        "rust",
        Language::Rust,
        |i| format!("m{i:05}"),
        |i| Utf8PathBuf::from(format!("src/m{i:05}.rs")),
        // A bare root, which is the path that used to rebuild the whole module set
        // before falling through to the declared-crate check.
        |i| format!("m{i:05}::Thing"),
        "src/lib.rs",
    );
}

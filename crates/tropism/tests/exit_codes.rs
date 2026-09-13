//! The exit-code contract, run against the built binary.
//!
//! `0` clean, `1` findings at or above `--fail-on`, `2` could not run. Conflating
//! `0` and `2` means a broken configuration looks like a passing build, which is
//! exactly what issue #43 reported: a ruleset using `layers` exited 0 from both
//! commands. See `design/05-interfaces.md`.

use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!(
        "{}/../tropism-lang/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn tropism(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tropism"))
        .args(args)
        .output()
        .expect("tropism did not start")
}

fn check(root: &str) -> Output {
    // The scan root goes through `--path`: a positional argument is a file.
    tropism(&["check", "--path", root, "--format", "text"])
}

fn analyze(root: &str) -> Output {
    tropism(&["analyze", root, "--format", "text", "--fail-on", "error"])
}

#[test]
fn a_ruleset_that_does_not_load_exits_2_from_both_commands() {
    let root = fixture("rules-layers");
    for (command, output) in [("check", check(&root)), ("analyze", analyze(&root))] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{command}: {stderr}");
        assert!(stderr.contains("`layers`"), "{command}: {stderr}");
    }
}

#[test]
fn the_same_violation_under_an_implemented_rule_exits_1_from_both_commands() {
    let root = fixture("rules-allow-only");
    for (command, output) in [("check", check(&root)), ("analyze", analyze(&root))] {
        assert_eq!(
            output.status.code(),
            Some(1),
            "{command}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

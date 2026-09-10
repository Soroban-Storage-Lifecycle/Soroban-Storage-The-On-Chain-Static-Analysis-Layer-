//! End-to-end rule tests over fixture source files.

use std::collections::HashSet;
use std::path::Path;

use soroban_storage_lints::{lint_source, Finding, LintOptions, Severity};

fn lint_fixture(name: &str) -> Vec<Finding> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let source = std::fs::read_to_string(&path).expect("fixture exists");
    lint_source(&path, &source, &LintOptions::default())
}

fn rules_of(findings: &[Finding]) -> Vec<(&'static str, Severity)> {
    findings.iter().map(|f| (f.rule, f.severity)).collect()
}

fn assert_has(findings: &[Finding], rule: &'static str, severity: Severity) {
    assert!(
        findings
            .iter()
            .any(|f| f.rule == rule && f.severity == severity),
        "expected finding [{severity:?}] {rule}, got: {}",
        findings
            .iter()
            .map(|f| format!("{}:{}", f.rule, f.severity.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn assert_not_has(findings: &[Finding], rule: &'static str) {
    assert!(
        !findings.iter().any(|f| f.rule == rule),
        "did not expect {rule}, got: {}",
        findings
            .iter()
            .map(|f| format!("{}:{}", f.rule, f.severity.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

#[test]
fn clean_fixture_has_no_findings() {
    let findings = lint_fixture("clean.rs");
    assert_eq!(findings, Vec::new(), "clean fixture produced findings");
}

#[test]
fn temporary_critical_is_flagged() {
    let findings = lint_fixture("temporary_critical.rs");
    assert_has(&findings, "temporary_critical_type", Severity::Error);
    // `Balance` derives Critical -> flagged.
    // `TokenBalance` matches the naming heuristic -> flagged.
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == "temporary_critical_type")
            .count(),
        2,
        "expected both the derived-Critical and heuristic cases to be flagged"
    );
}

#[test]
fn lifecycle_mismatch_is_flagged() {
    let findings = lint_fixture("lifecycle_mismatch.rs");
    assert_has(&findings, "lifecycle_mismatch", Severity::Error);
    // Both directions (persistent key via temporary, temporary key via persistent).
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == "lifecycle_mismatch")
            .count(),
        2
    );
}

#[test]
fn missing_ttl_extension_is_flagged() {
    let findings = lint_fixture("missing_ttl.rs");
    assert_has(&findings, "missing_ttl_extension", Severity::Warning);
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == "missing_ttl_extension")
            .count(),
        2,
        "both un-bumped writes flagged; the bumped one must stay quiet"
    );
}

#[test]
fn instance_bloat_is_flagged() {
    let findings = lint_fixture("instance_bloat.rs");
    assert_has(&findings, "instance_storage_bloat", Severity::Warning);
    // Vec value, 4-field struct, and loop write; the 2-field struct is fine.
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == "instance_storage_bloat")
            .count(),
        3
    );
}

#[test]
fn temp_ttl_exceeded_is_flagged() {
    let findings = lint_fixture("temp_ttl_exceeded.rs");
    assert_has(&findings, "temporary_ttl_exceeded", Severity::Warning);
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == "temporary_ttl_exceeded")
            .count(),
        1
    );
}

#[test]
fn ignored_rules_are_skipped() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join("temporary_critical.rs");
    let source = std::fs::read_to_string(&path).unwrap();
    let opts = LintOptions {
        ignored_rules: HashSet::from(["temporary_critical_type".to_string()]),
        ..LintOptions::default()
    };
    let findings = lint_source(&path, &source, &opts);
    assert_not_has(&findings, "temporary_critical_type");
}

#[test]
fn deny_warnings_config_is_visible() {
    let opts = LintOptions {
        deny_warnings: true,
        ..LintOptions::default()
    };
    assert!(opts.deny_warnings);
}

#[test]
fn findings_carry_usable_spans() {
    let findings = lint_fixture("temporary_critical.rs");
    for f in &findings {
        assert!(f.line > 0, "finding must have a line");
        assert!(f.column > 0, "finding must have a column");
        assert!(!f.snippet.is_empty(), "finding must carry a source snippet");
        assert_eq!(f.path.extension().unwrap(), "rs");
    }
}

#[test]
fn rules_of_ordering_is_stable() {
    let a = lint_fixture("lifecycle_mismatch.rs");
    let b = lint_fixture("lifecycle_mismatch.rs");
    assert_eq!(rules_of(&a), rules_of(&b));
}

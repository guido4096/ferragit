// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

#[test]
fn matches_reference_for_the_milestone_workflows() {
    let candidate = std::path::Path::new(env!("CARGO_BIN_EXE_ferragit"));
    let reference = std::env::var_os("FERRAGIT_REFERENCE").unwrap_or_else(|| "git".into());
    let report =
        git_conformance::run_suite(candidate, std::path::Path::new(&reference), true).unwrap();
    for case in &report.cases {
        if !case.passed {
            eprintln!("{}: {}", case.name, case.detail);
        }
    }
    assert_eq!(report.failed, 0, "fixture: {:?}", report.retained_fixture);
}

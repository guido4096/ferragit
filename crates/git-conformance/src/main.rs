// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

use std::{env, fs, path::PathBuf};
fn main() {
    if let Err(error) = run() {
        eprintln!("conformance: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut candidate = None;
    let mut reference = PathBuf::from("git");
    let mut report = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--candidate") => {
                candidate = Some(PathBuf::from(args.next().ok_or("missing candidate")?))
            }
            Some("--reference") => {
                reference = PathBuf::from(args.next().ok_or("missing reference")?)
            }
            Some("--report") => report = Some(PathBuf::from(args.next().ok_or("missing report")?)),
            _ => return Err(
                "usage: git-conformance --candidate <rugit> [--reference <git>] [--report <file>]"
                    .into(),
            ),
        }
    }
    let result = git_conformance::run_suite(
        &candidate.ok_or("--candidate is required")?,
        &reference,
        true,
    )?;
    println!(
        "{} passed; {} failed; {} total against {} on {}/{}",
        result.passed,
        result.failed,
        result.total,
        result.reference_version,
        result.platform,
        result.architecture
    );
    for case in &result.cases {
        if !case.passed {
            eprintln!("FAIL {}: {}", case.name, case.detail);
        }
    }
    if let Some(path) = report {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }
    if result.failed != 0 {
        return Err(format!(
            "{} comparisons failed; fixture: {:?}",
            result.failed, result.retained_fixture
        )
        .into());
    }
    Ok(())
}

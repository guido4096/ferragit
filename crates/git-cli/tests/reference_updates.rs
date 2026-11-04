// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Independent reference/candidate mutations, including observable partial failures.
use git_conformance::{executable, invoke, Fixture};
use git_core::{
    identity::Identity,
    object::{HashKind, ObjectId},
    refs::{Expected, Update},
    repository::Repository,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Barrier},
    thread,
};

struct Pair {
    _fixture: Fixture,
    left: PathBuf,
    right: PathBuf,
    home: PathBuf,
    reference: PathBuf,
    candidate: PathBuf,
    hash: HashKind,
    bare: bool,
}

impl Pair {
    fn new(hash: HashKind, bare: bool) -> Self {
        let fixture = Fixture::new().unwrap();
        let left = fixture.path.join("reference");
        let right = fixture.path.join("candidate");
        let home = fixture.path.join("home");
        for path in [&left, &right, &home] {
            fs::create_dir(path).unwrap();
        }
        fs::write(home.join("empty-config"), b"").unwrap();
        let pair = Self {
            reference: executable(
                &std::env::var_os("FERRAGIT_REFERENCE").unwrap_or_else(|| "git".into()),
            )
            .unwrap(),
            candidate: executable(OsStr::new(env!("CARGO_BIN_EXE_ferragit"))).unwrap(),
            _fixture: fixture,
            left,
            right,
            home,
            hash,
            bare,
        };
        let format = format!("--object-format={}", hash.name());
        let mut args = vec!["init", "-q", "--template=", "-b", "main", &format];
        if bare {
            args.push("--bare");
        }
        pair.compare(&args, b"");
        pair
    }
    fn git(&self, candidate: bool) -> PathBuf {
        let root = if candidate { &self.right } else { &self.left };
        if self.bare {
            root.clone()
        } else {
            root.join(".git")
        }
    }
    fn run(&self, candidate: bool, args: &[&str], input: &[u8]) -> Output {
        invoke(
            if candidate {
                &self.candidate
            } else {
                &self.reference
            },
            if candidate { &self.right } else { &self.left },
            &self.home,
            args,
            input,
            candidate,
        )
        .unwrap()
    }
    fn compare(&self, args: &[&str], input: &[u8]) -> Output {
        let reference = self.run(false, args, input);
        let candidate = self.run(true, args, input);
        assert_eq!(
            reference.status, candidate.status,
            "status: {args:?}; reference={reference:?}; candidate={candidate:?}"
        );
        assert_eq!(reference.stdout, candidate.stdout, "stdout: {args:?}");
        assert_eq!(reference.stderr, candidate.stderr, "stderr: {args:?}");
        candidate
    }
    fn success(&self, args: &[&str], input: &[u8]) -> Output {
        let out = self.compare(args, input);
        assert!(out.status.success(), "{args:?}: {out:?}");
        out
    }
    fn object(&self, kind: &str, data: &[u8]) -> String {
        String::from_utf8(
            self.success(
                &["hash-object", "-w", "--literally", "-t", kind, "--stdin"],
                data,
            )
            .stdout,
        )
        .unwrap()
        .trim()
        .to_owned()
    }
    fn commits(&self) -> [String; 3] {
        let tree = self.object("tree", b"");
        ["one", "two", "three"].map(|message| self.object("commit", format!("tree {tree}\nauthor Conformance Author <author@example.invalid> 1700000000 +0000\ncommitter Conformance Committer <committer@example.invalid> 1700000000 +0000\n\n{message}\n").as_bytes()))
    }
    fn config(&self, value: &str) {
        for candidate in [false, true] {
            let path = self.git(candidate).join("config");
            let source = fs::read_to_string(&path).unwrap();
            let source = source
                .lines()
                .filter(|line| !line.to_ascii_lowercase().contains("logallrefupdates"))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(
                path,
                format!("{source}\n[core]\nlogallrefupdates = {value}\n"),
            )
            .unwrap();
        }
    }
    fn state(&self, candidate: bool) -> BTreeMap<PathBuf, Vec<u8>> {
        let git = self.git(candidate);
        let mut result = BTreeMap::new();
        for item in ["HEAD", "packed-refs", "refs", "logs"] {
            snapshot(&git, &git.join(item), &mut result);
        }
        result
    }
    fn same_state(&self) {
        assert_eq!(self.state(false), self.state(true));
    }
    fn no_locks(&self) {
        let mut files = BTreeMap::new();
        let git = self.git(true);
        snapshot(&git, &git, &mut files);
        assert!(
            !files
                .keys()
                .any(|path| path.extension().is_some_and(|ext| ext == "lock")),
            "leaked locks: {:?}",
            files.keys()
        );
    }
}

fn snapshot(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            snapshot(root, &entry.unwrap().path(), files);
        }
    } else if path.is_file() {
        files.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            fs::read(path).unwrap(),
        );
    }
}

#[test]
fn commits_and_packed_ref_updates_have_identical_reflogs() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash, false);
        for revision in 0..2 {
            for root in [&pair.left, &pair.right] {
                fs::write(root.join("file"), format!("revision {revision}\n")).unwrap();
            }
            pair.success(&["add", "-A"], b"");
            pair.success(
                &[
                    "commit",
                    "-q",
                    "-m",
                    &format!("revision {revision}\nsecond line\n\nbody"),
                ],
                b"",
            );
            pair.same_state();
        }
        let current = String::from_utf8(pair.success(&["rev-parse", "HEAD"], b"").stdout)
            .unwrap()
            .trim()
            .to_owned();
        let [a, b, _] = pair.commits();
        let zero = "0".repeat(hash.hex_len());
        pair.success(
            &[
                "update-ref",
                "-m",
                "  topic\t  created\nreason  ",
                "refs/heads/topic",
                &a,
                &zero,
            ],
            b"",
        );
        pair.success(&["update-ref", "refs/heads/topic", &b, &a], b"");
        pair.success(&["update-ref", "HEAD", &current, &current], b"");
        pair.success(&["update-ref", "refs/heads/main", &current], b"");
        pair.same_state();
        for candidate in [false, true] {
            let out = invoke(
                &pair.reference,
                if candidate { &pair.right } else { &pair.left },
                &pair.home,
                &["pack-refs", "--all"],
                b"",
                false,
            )
            .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        pair.success(
            &["update-ref", "-m", "packed branch", "HEAD", &a, &current],
            b"",
        );
        assert!(pair.git(true).join("refs/heads/main").is_file());
        pair.success(&["update-ref", "refs/heads/main", &b, &a], b"");
        pair.same_state();
        let fsck = invoke(
            &pair.reference,
            &pair.right,
            &pair.home,
            &["fsck", "--strict", "--no-dangling"],
            b"",
            false,
        )
        .unwrap();
        assert!(fsck.status.success(), "{fsck:?}");
        pair.no_locks();
    }
}

#[test]
fn expected_values_protect_refs_and_match_reference_diagnostics() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash, false);
        let [a, b, c] = pair.commits();
        let zero = "0".repeat(hash.hex_len());
        assert!(!pair
            .compare(&["update-ref", "refs/heads/main", &a, &b], b"")
            .status
            .success());
        pair.success(&["update-ref", "refs/heads/main", &a, &zero], b"");
        for (reference, expected) in [
            ("refs/heads/main", zero.as_str()),
            ("refs/heads/main", b.as_str()),
            ("refs/heads/missing", c.as_str()),
        ] {
            let before = pair.state(true);
            assert!(!pair
                .compare(&["update-ref", reference, &b, expected], b"")
                .status
                .success());
            assert_eq!(before, pair.state(true));
            pair.same_state();
            pair.no_locks();
        }
        let missing = "1".repeat(hash.hex_len());
        assert!(!pair
            .compare(&["update-ref", "refs/heads/main", &missing], b"")
            .status
            .success());
        let blob = pair.object("blob", b"not a commit");
        assert!(!pair
            .compare(&["update-ref", "refs/heads/main", &blob], b"")
            .status
            .success());
        pair.success(&["update-ref", "refs/tags/blob", &blob, ""], b"");
        pair.same_state();
        pair.no_locks();
    }
}

#[test]
fn symbolic_chains_and_detached_head_updates_interchange() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash, false);
        let [a, b, _] = pair.commits();
        for candidate in [false, true] {
            fs::write(pair.git(candidate).join("HEAD"), b"ref: refs/heads/link\n").unwrap();
            fs::write(
                pair.git(candidate).join("refs/heads/link"),
                b"ref: refs/heads/main\n",
            )
            .unwrap();
        }
        pair.success(&["update-ref", "HEAD", &a], b"");
        pair.same_state();
        pair.success(&["update-ref", "HEAD", &b, &a], b"");
        pair.same_state();
        for candidate in [false, true] {
            fs::write(pair.git(candidate).join("HEAD"), format!("{a}\n")).unwrap();
        }
        pair.success(&["update-ref", "HEAD", &b, &a], b"");
        pair.same_state();
        pair.no_locks();
    }
}

#[test]
fn reflog_policy_covers_bare_disabled_existing_and_always_logs() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for bare in [false, true] {
            let pair = Pair::new(hash, bare);
            let [a, b, c] = pair.commits();
            pair.success(&["update-ref", "HEAD", &a], b"");
            pair.same_state();
            assert_eq!(pair.git(true).join("logs/HEAD").exists(), !bare);
            pair.config("false");
            pair.success(&["update-ref", "HEAD", &b], b"");
            pair.same_state();
            pair.success(&["update-ref", "refs/heads/no-log", &a], b"");
            assert!(!pair.git(true).join("logs/refs/heads/no-log").exists());
            pair.success(&["update-ref", "refs/tags/tag", &a], b"");
            assert!(!pair.git(true).join("logs/refs/tags/tag").exists());
            pair.config("always");
            pair.success(&["update-ref", "-m", "all refs", "refs/tags/tag", &c], b"");
            assert!(pair.git(true).join("logs/refs/tags/tag").is_file());
            pair.same_state();
            pair.no_locks();
        }
    }
}

#[test]
fn reflog_failure_effects_match_the_files_backend() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for reference in ["HEAD", "refs/heads/main"] {
            for blocked in ["HEAD", "refs/heads/main"] {
                let pair = Pair::new(hash, false);
                let [a, b, _] = pair.commits();
                pair.success(&["update-ref", "HEAD", &a], b"");
                for candidate in [false, true] {
                    let path = pair.git(candidate).join("logs").join(blocked);
                    fs::remove_file(&path).unwrap();
                    fs::create_dir(&path).unwrap();
                    fs::write(path.join("keep"), b"existing log namespace").unwrap();
                }
                assert!(!pair
                    .compare(&["update-ref", reference, &b], b"")
                    .status
                    .success());
                pair.same_state();
                pair.no_locks();
                let current = Repository::discover(&pair.right)
                    .unwrap()
                    .resolve("refs/heads/main")
                    .unwrap()
                    .to_string();
                assert_eq!(
                    current,
                    if reference == "refs/heads/main" && blocked == "HEAD" {
                        b
                    } else {
                        a
                    }
                );
            }
        }
    }
}

#[test]
fn stale_locks_and_unsupported_inputs_preserve_existing_state() {
    let pair = Pair::new(HashKind::Sha1, false);
    let [a, b, _] = pair.commits();
    pair.success(&["update-ref", "HEAD", &a], b"");
    for name in ["HEAD.lock", "refs/heads/main.lock"] {
        let lock = pair.git(true).join(name);
        fs::write(&lock, b"other writer").unwrap();
        let before = pair.state(true);
        assert!(!pair
            .run(true, &["update-ref", "HEAD", &b], b"")
            .status
            .success());
        assert_eq!(before, pair.state(true));
        assert_eq!(fs::read(&lock).unwrap(), b"other writer");
        fs::remove_file(lock).unwrap();
        pair.no_locks();
    }
    let zero = "0".repeat(40);
    for args in [
        vec!["update-ref", "-d", "refs/heads/main"],
        vec!["update-ref", "--stdin"],
        vec!["update-ref", "refs/heads/main", &zero],
        vec!["update-ref", "refs/heads/../config", &b],
    ] {
        let before = pair.state(true);
        assert!(!pair.run(true, &args, b"").status.success());
        assert_eq!(before, pair.state(true));
        pair.no_locks();
    }
    fs::write(
        pair.git(true).join("refs/heads/cycle"),
        b"ref: refs/heads/cycle\n",
    )
    .unwrap();
    let before = pair.state(true);
    assert!(!pair
        .run(true, &["update-ref", "refs/heads/cycle", &b], b"")
        .status
        .success());
    assert_eq!(before, pair.state(true));
    pair.no_locks();
    let hooks = pair.git(true).join("hooks");
    fs::create_dir(&hooks).unwrap();
    fs::write(hooks.join("reference-transaction"), b"must not execute").unwrap();
    let before = pair.state(true);
    assert!(!pair
        .run(true, &["update-ref", "HEAD", &b], b"")
        .status
        .success());
    assert_eq!(before, pair.state(true));
    pair.no_locks();
}

#[test]
fn competing_expected_value_writers_cannot_both_replace_the_ref() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash, false);
        let [a, b, c] = pair.commits();
        pair.success(&["update-ref", "HEAD", &a], b"");
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = [b, c]
            .into_iter()
            .map(|new| {
                let (binary, root, home, old, barrier) = (
                    pair.candidate.clone(),
                    pair.right.clone(),
                    pair.home.clone(),
                    a.clone(),
                    Arc::clone(&barrier),
                );
                thread::spawn(move || {
                    barrier.wait();
                    (
                        new.clone(),
                        invoke(
                            &binary,
                            &root,
                            &home,
                            &["update-ref", "refs/heads/main", &new, &old],
                            b"",
                            true,
                        )
                        .unwrap(),
                    )
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let winners: Vec<_> = results
            .iter()
            .filter(|(_, output)| output.status.success())
            .collect();
        assert_eq!(winners.len(), 1, "{results:?}");
        assert_eq!(
            Repository::discover(&pair.right)
                .unwrap()
                .resolve("HEAD")
                .unwrap()
                .to_string(),
            winners[0].0
        );
        for name in ["HEAD", "refs/heads/main"] {
            assert_eq!(
                fs::read_to_string(pair.git(true).join("logs").join(name))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        pair.no_locks();
    }
}

#[test]
fn publication_failure_reports_error_and_releases_owned_locks() {
    let pair = Pair::new(HashKind::Sha1, false);
    let [a, b, _] = pair.commits();
    pair.success(&["update-ref", "HEAD", &a], b"");
    let repo = Repository::discover(&pair.right).unwrap();
    let update = Update::lock(
        &repo,
        "HEAD",
        Expected::Value(ObjectId::parse(pair.hash, &a).unwrap()),
    )
    .unwrap();
    let path = repo.git_dir.join("refs/heads/main");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let identity = Identity {
        name: "Conformance Committer".into(),
        email: "committer@example.invalid".into(),
        date: "1700000000 +0000".into(),
    };
    assert!(update
        .finish(
            &ObjectId::parse(pair.hash, &b).unwrap(),
            || Ok(identity),
            b"publication failure"
        )
        .is_err());
    assert!(path.is_dir());
    pair.no_locks();
    // Logs can precede a failed publication; the implementation must expose the
    // failure instead of pretending to roll back all already-visible effects.
    assert!(fs::read_to_string(repo.git_dir.join("logs/HEAD"))
        .unwrap()
        .contains("publication failure"));
}

#[test]
fn disabled_logging_does_not_require_a_committer_identity() {
    let pair = Pair::new(HashKind::Sha1, false);
    let [a, _, _] = pair.commits();
    pair.config("false");
    for candidate in [false, true] {
        let mut command = Command::new(if candidate {
            &pair.candidate
        } else {
            &pair.reference
        });
        command
            .current_dir(if candidate { &pair.right } else { &pair.left })
            .env_clear();
        for key in [
            "SYSTEMROOT",
            "SystemRoot",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TMP",
            "TEMP",
            "TMPDIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("HOME", &pair.home)
            .env("USERPROFILE", &pair.home)
            .env("PATH", &pair.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", pair.home.join("empty-config"))
            .env("GIT_EXEC_PATH", &pair.home);
        let output = command
            .args(["update-ref", "refs/heads/main", &a])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    pair.same_state();
    pair.no_locks();
}

#[cfg(unix)]
#[test]
fn symlink_write_paths_cannot_change_outside_files() {
    use std::os::unix::fs::symlink;
    let pair = Pair::new(HashKind::Sha1, false);
    let [a, b, _] = pair.commits();
    pair.success(&["update-ref", "HEAD", &a], b"");
    let outside = pair._fixture.path.join("outside");
    fs::write(&outside, format!("{a}\n")).unwrap();
    let path = pair.git(true).join("refs/heads/main");
    fs::remove_file(path.clone()).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(!pair
        .run(true, &["update-ref", "HEAD", &b], b"")
        .status
        .success());
    assert_eq!(fs::read(&outside).unwrap(), format!("{a}\n").as_bytes());
    pair.no_locks();
}

#[test]
fn commit_log_failure_preserves_the_index_and_matches_git_state() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash, false);
        for root in [&pair.left, &pair.right] {
            fs::write(root.join("file"), b"first\n").unwrap();
        }
        pair.success(&["add", "-A"], b"");
        pair.success(&["commit", "-q", "-m", "first"], b"");
        for root in [&pair.left, &pair.right] {
            fs::write(root.join("file"), b"second\n").unwrap();
        }
        pair.success(&["add", "-A"], b"");
        let index = fs::read(pair.git(true).join("index")).unwrap();
        let old = Repository::discover(&pair.right)
            .unwrap()
            .resolve("HEAD")
            .unwrap();
        for candidate in [false, true] {
            let path = pair.git(candidate).join("logs/refs/heads/main");
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            fs::write(path.join("keep"), b"existing log namespace").unwrap();
        }
        assert!(!pair
            .compare(&["commit", "-q", "-m", "second"], b"")
            .status
            .success());
        assert_eq!(fs::read(pair.git(true).join("index")).unwrap(), index);
        assert_eq!(
            Repository::discover(&pair.right)
                .unwrap()
                .resolve("HEAD")
                .unwrap(),
            old
        );
        pair.same_state();
        pair.no_locks();
    }
}

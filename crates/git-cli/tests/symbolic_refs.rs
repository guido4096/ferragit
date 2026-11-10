// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Symbolic-reference command comparisons on independent disposable fixtures.
use git_conformance::{executable, invoke, Fixture};
use git_core::object::HashKind;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Output,
};

struct Pair {
    _fixture: Fixture,
    bare: bool,
    roots: [PathBuf; 2],
    home: PathBuf,
    reference: PathBuf,
    candidate: PathBuf,
}
impl Pair {
    fn new(hash: HashKind) -> Self {
        Self::with_bare(hash, false)
    }
    fn with_bare(hash: HashKind, bare: bool) -> Self {
        let fixture = Fixture::new().unwrap();
        let roots = [
            fixture.path.join("reference"),
            fixture.path.join("candidate"),
        ];
        let home = fixture.path.join("home");
        for path in [&roots[0], &roots[1], &home] {
            fs::create_dir(path).unwrap();
        }
        let reference =
            executable(&std::env::var_os("FERRAGIT_REFERENCE").unwrap_or_else(|| "git".into()))
                .unwrap();
        let candidate = executable(OsStr::new(env!("CARGO_BIN_EXE_ferragit"))).unwrap();
        let pair = Self {
            _fixture: fixture,
            bare,
            roots,
            home,
            reference,
            candidate,
        };
        for root in &pair.roots {
            let format = format!("--object-format={}", hash.name());
            let mut args = vec!["init", "-q", "--template=", "-b", "main", &format];
            if bare {
                args.push("--bare");
            }
            let out = invoke(&pair.reference, root, &pair.home, &args, b"", false).unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        pair
    }
    fn git(&self, side: usize) -> PathBuf {
        if self.bare {
            self.roots[side].clone()
        } else {
            self.roots[side].join(".git")
        }
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        for side in 0..2 {
            let path = self.git(side).join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }
    fn state(&self, side: usize) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            if path.is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                for entry in fs::read_dir(path).unwrap() {
                    walk(root, &entry.unwrap().path(), result);
                }
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    Some(fs::read(path).unwrap()),
                );
            }
        }
        let mut result = BTreeMap::new();
        walk(&self.roots[side], &self.git(side), &mut result);
        result
    }
    fn run(&self, side: usize, args: &[&str]) -> Output {
        invoke(
            if side == 0 {
                &self.reference
            } else {
                &self.candidate
            },
            &self.roots[side],
            &self.home,
            args,
            b"",
            side == 1,
        )
        .unwrap()
    }
    fn read(&self, args: &[&str], expected_code: i32, stdout: &[u8]) {
        let before = [self.state(0), self.state(1)];
        let left = self.run(0, args);
        let right = self.run(1, args);
        assert_eq!(
            left.status.code(),
            Some(expected_code),
            "oracle: {args:?}: {left:?}"
        );
        assert_eq!(left.stdout, stdout, "oracle: {args:?}");
        assert_eq!(left.status, right.status, "status: {args:?}: {right:?}");
        assert_eq!(left.stdout, right.stdout, "stdout: {args:?}");
        assert_eq!(left.stderr, right.stderr, "stderr: {args:?}");
        assert_eq!(before[0], self.state(0), "oracle read mutated state");
        assert_eq!(before[1], self.state(1), "candidate read mutated state");
    }
}

#[test]
fn reads_unborn_symbolic_refs_and_quiet_direct_or_missing_refs() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash);
        pair.read(&["symbolic-ref", "HEAD"], 0, b"refs/heads/main\n");
        pair.read(&["symbolic-ref", "--quiet", "refs/heads/missing"], 1, b"");
        pair.read(&["symbolic-ref", "refs/heads/missing"], 128, b"");
        pair.write(
            "refs/heads/main",
            format!("{}\n", "a".repeat(hash.hex_len())).as_bytes(),
        );
        pair.read(&["symbolic-ref", "-q", "refs/heads/main"], 1, b"");
        pair.read(&["symbolic-ref", "refs/heads/main"], 128, b"");
        pair.write(
            "HEAD",
            format!("{}\n", "a".repeat(hash.hex_len())).as_bytes(),
        );
        pair.read(&["symbolic-ref", "-q", "HEAD"], 1, b"");
        pair.read(&["symbolic-ref", "HEAD"], 128, b"");
    }
}

#[test]
fn follows_chains_with_the_reference_depth_boundary_and_one_level_reads() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for links in 1..=6 {
            let pair = Pair::new(hash);
            pair.write("HEAD", b"ref: refs/heads/link1\n");
            for n in 1..links {
                pair.write(
                    &format!("refs/heads/link{n}"),
                    format!("ref: refs/heads/link{}\n", n + 1).as_bytes(),
                );
            }
            pair.read(
                &["symbolic-ref", "--no-recurse", "HEAD"],
                0,
                b"refs/heads/link1\n",
            );
            let target = format!("refs/heads/link{links}\n");
            pair.read(
                &["symbolic-ref", "--recurse", "HEAD"],
                if links < 5 { 0 } else { 128 },
                if links < 5 { target.as_bytes() } else { b"" },
            );
        }
        let pair = Pair::new(hash);
        pair.write("HEAD", b"ref: refs/heads/loop\n");
        pair.write("refs/heads/loop", b"ref: refs/heads/loop\n");
        pair.read(&["symbolic-ref", "HEAD"], 128, b"");
        pair.read(&["symbolic-ref", "-q", "HEAD"], 128, b"");
        pair.read(
            &["symbolic-ref", "--no-recurse", "HEAD"],
            0,
            b"refs/heads/loop\n",
        );
    }
}

#[test]
fn distinguishes_malformed_refs_from_direct_packed_refs() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash);
        pair.write(
            "packed-refs",
            format!(
                "# pack-refs with: peeled fully-peeled sorted \n{} refs/heads/packed\n",
                "b".repeat(hash.hex_len())
            )
            .as_bytes(),
        );
        pair.read(&["symbolic-ref", "-q", "refs/heads/packed"], 1, b"");
        pair.read(&["symbolic-ref", "refs/heads/packed"], 128, b"");
        for bytes in [
            b"broken\n".as_slice(),
            b"\xff\n",
            b"ref: refs/heads/../config\n",
            b"ref: \n",
        ] {
            pair.write("refs/heads/broken", bytes);
            pair.read(&["symbolic-ref", "refs/heads/broken"], 128, b"");
            pair.read(&["symbolic-ref", "-q", "refs/heads/broken"], 128, b"");
        }
        pair.read(&["symbolic-ref", "refs/heads/../config"], 128, b"");
        pair.write(
            "refs/heads/blocker",
            format!("{}\n", "a".repeat(hash.hex_len())).as_bytes(),
        );
        pair.write("refs/heads/alias", b"ref: refs/heads/blocker/child\n");
        pair.read(
            &["symbolic-ref", "refs/heads/alias"],
            0,
            b"refs/heads/blocker/child\n",
        );
        pair.write("refs/heads/alias", b"ref:\trefs/heads/unborn  \n");
        pair.read(
            &["symbolic-ref", "--", "refs/heads/alias"],
            0,
            b"refs/heads/unborn\n",
        );
        pair.write("refs/heads/alias", b"ref:\x0brefs/heads/unborn\x0b\n");
        pair.read(&["symbolic-ref", "refs/heads/alias"], 128, b"");
        pair.write("refs/heads/alias", b"ref:\x0crefs/heads/unborn\n");
        pair.read(&["symbolic-ref", "refs/heads/alias"], 128, b"");
        pair.write("refs/heads/alias", b"ref: refs/heads/unborn \0ignored\n");
        pair.read(&["symbolic-ref", "refs/heads/alias"], 128, b"");
        pair.read(
            &["symbolic-ref", "--no-recurse", "refs/heads/alias"],
            0,
            b"refs/heads/unborn \n",
        );
    }
}

impl Pair {
    fn compare(&self, args: &[&str], code: i32) {
        let left = self.run(0, args);
        let right = self.run(1, args);
        assert_eq!(left.status.code(), Some(code), "oracle: {args:?}: {left:?}");
        assert_eq!(left.status, right.status, "status: {args:?}: {right:?}");
        assert_eq!(left.stdout, right.stdout, "stdout: {args:?}");
        assert_eq!(left.stderr, right.stderr, "stderr: {args:?}");
        assert_eq!(self.state(0), self.state(1), "state: {args:?}");
    }
    fn object(&self, kind: &str, bytes: &[u8]) -> String {
        let mut result = None;
        for root in &self.roots {
            let output = invoke(
                &self.reference,
                root,
                &self.home,
                &["hash-object", "-w", "--literally", "-t", kind, "--stdin"],
                bytes,
                false,
            )
            .unwrap();
            assert!(output.status.success());
            let value = String::from_utf8(output.stdout).unwrap().trim().to_owned();
            if let Some(previous) = &result {
                assert_eq!(previous, &value);
            }
            result = Some(value);
        }
        result.unwrap()
    }
    fn commits(&self) -> [String; 2] {
        let tree = self.object("tree", b"");
        ["one", "two"].map(|subject| self.object("commit", format!("tree {tree}\nauthor A <a@b> 1700000000 +0000\ncommitter A <a@b> 1700000000 +0000\n\n{subject}\n").as_bytes()))
    }
    fn setup_ref(&self, name: &str, value: &str) {
        for side in 0..2 {
            assert!(invoke(
                &self.reference,
                &self.roots[side],
                &self.home,
                &["update-ref", name, value],
                b"",
                false
            )
            .unwrap()
            .status
            .success());
        }
    }
    fn config(&self, value: &str) {
        for side in 0..2 {
            let path = self.git(side).join("config");
            let text = fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|line| !line.to_ascii_lowercase().contains("logallrefupdates"))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(path, format!("{text}\n[core]\nlogallrefupdates={value}\n")).unwrap();
        }
    }
}

#[test]
fn writes_named_symbolic_refs_without_altering_their_destinations() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for mode in [
            "unborn",
            "head",
            "alias",
            "current-branch",
            "packed",
            "existing-symbolic",
            "current-symbolic",
            "empty-directory",
            "self-cycle",
        ] {
            let pair = Pair::new(hash);
            let [first, second] = pair.commits();
            pair.setup_ref("refs/heads/main", &first);
            pair.setup_ref("refs/heads/other", &second);
            let name = match mode {
                "alias" | "existing-symbolic" | "current-symbolic" | "empty-directory" => {
                    "refs/heads/alias"
                }
                "current-branch" | "packed" | "self-cycle" => "refs/heads/main",
                _ => "HEAD",
            };
            if mode == "packed" {
                for side in 0..2 {
                    assert!(invoke(
                        &pair.reference,
                        &pair.roots[side],
                        &pair.home,
                        &["pack-refs", "--all"],
                        b"",
                        false
                    )
                    .unwrap()
                    .status
                    .success());
                }
            }
            if mode == "existing-symbolic" || mode == "current-symbolic" {
                pair.write(name, b"ref: refs/heads/main\n");
            }
            if mode == "current-symbolic" {
                pair.write("HEAD", b"ref: refs/heads/alias\n");
            }
            if mode == "empty-directory" {
                for side in 0..2 {
                    fs::create_dir(pair.git(side).join(name)).unwrap();
                }
            }
            let target = match mode {
                "unborn" => "refs/heads/unborn",
                "self-cycle" => "refs/heads/main",
                _ => "refs/heads/other",
            };
            pair.compare(
                &[
                    "symbolic-ref",
                    "-m",
                    " move\tto\nother\x0c\x0b ",
                    name,
                    target,
                ],
                0,
            );
            assert_eq!(
                fs::read(pair.git(1).join(name)).unwrap(),
                format!("ref: {target}\n").as_bytes()
            );
            assert_eq!(
                git_core::repository::Repository::discover(&pair.roots[1])
                    .unwrap()
                    .resolve("refs/heads/other")
                    .unwrap()
                    .to_string(),
                second
            );
            if mode != "self-cycle" {
                pair.read(
                    &["symbolic-ref", "--no-recurse", name],
                    0,
                    format!("{target}\n").as_bytes(),
                );
            }
        }
    }
}

#[test]
fn symbolic_writes_match_reflog_policy_with_and_without_reasons() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for bare in [false, true] {
            for policy in ["true", "false", "always"] {
                let pair = Pair::with_bare(hash, bare);
                pair.config(policy);
                let [first, second] = pair.commits();
                pair.setup_ref("refs/heads/main", &first);
                pair.setup_ref("refs/heads/other", &second);
                pair.compare(&["symbolic-ref", "HEAD", "refs/heads/other"], 0);
                pair.compare(
                    &[
                        "symbolic-ref",
                        "-m",
                        "reason",
                        "refs/custom/alias",
                        "refs/heads/main",
                    ],
                    0,
                );
                pair.compare(
                    &["symbolic-ref", "-m", "again", "HEAD", "refs/heads/other"],
                    0,
                );
                pair.compare(
                    &["symbolic-ref", "-m", "unborn", "HEAD", "refs/heads/unborn"],
                    0,
                );
                pair.write("logs/HEAD", b"");
                pair.compare(
                    &[
                        "symbolic-ref",
                        "-m",
                        "existing log",
                        "HEAD",
                        "refs/heads/main",
                    ],
                    0,
                );
            }
        }
    }
}

#[test]
fn symbolic_write_failures_preserve_the_reference_failure_boundary() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for blocked in ["source", "HEAD"] {
            let pair = Pair::new(hash);
            let [first, second] = pair.commits();
            pair.setup_ref("refs/heads/main", &first);
            pair.setup_ref("refs/heads/other", &second);
            for side in 0..2 {
                let path = pair.git(side).join(if blocked == "source" {
                    "logs/refs/heads/main"
                } else {
                    "logs/HEAD"
                });
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
                fs::write(path.join("keep"), b"preserve log").unwrap();
            }
            pair.compare(
                &[
                    "symbolic-ref",
                    "-m",
                    "blocked",
                    "refs/heads/main",
                    "refs/heads/other",
                ],
                1,
            );
            let actual = fs::read(pair.git(1).join("refs/heads/main")).unwrap();
            assert_eq!(
                actual,
                if blocked == "HEAD" {
                    b"ref: refs/heads/other\n".to_vec()
                } else {
                    format!("{first}\n").into_bytes()
                }
            );
        }
        let pair = Pair::new(hash);
        let before = pair.state(1);
        pair.compare(&["symbolic-ref", "-m", "", "HEAD", "refs/heads/other"], 128);
        pair.compare(&["symbolic-ref", "HEAD", "main"], 128);
        pair.compare(&["symbolic-ref", "HEAD", "refs/heads/../config"], 128);
        assert_eq!(before, pair.state(1));
    }
}

#[test]
fn symbolic_write_owns_its_locks_and_does_not_require_identity_for_unborn_targets() {
    use git_core::{identity::Identity, refs::write_symbolic, repository::Repository, Error};
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash);
        let repo = Repository::discover(&pair.roots[1]).unwrap();
        write_symbolic(
            &repo,
            "HEAD",
            "refs/heads/unborn",
            b"reason",
            || -> git_core::Result<Identity> { panic!("unborn target needs no identity") },
        )
        .unwrap();
        let lock = pair.git(1).join("HEAD.lock");
        fs::write(&lock, b"another writer").unwrap();
        let before = pair.state(1);
        assert!(
            write_symbolic(&repo, "HEAD", "refs/heads/other", b"", || Err(
                Error::Invalid("unused identity".into())
            ))
            .is_err()
        );
        assert_eq!(before, pair.state(1));
        assert_eq!(fs::read(lock).unwrap(), b"another writer");
    }
}

#[test]
fn competing_symbolic_and_object_id_writers_cannot_publish_while_prepared() {
    use git_core::{
        identity::Identity,
        refs::{write_symbolic, Expected, Update},
        repository::Repository,
    };
    use std::{sync::mpsc, thread, time::Duration};
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash);
        let [first, second] = pair.commits();
        pair.setup_ref("refs/heads/main", &first);
        pair.setup_ref("refs/heads/other", &second);
        let repo = Repository::discover(&pair.roots[1]).unwrap();
        let worker_root = pair.roots[1].clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let repo = Repository::discover(&worker_root).unwrap();
            write_symbolic(&repo, "HEAD", "refs/heads/other", b"race", || {
                ready_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                Ok(Identity {
                    name: "Conformance Committer".into(),
                    email: "committer@example.invalid".into(),
                    date: "1700000000 +0000".into(),
                })
            })
        });
        ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let before = pair.state(1);
        assert_eq!(
            fs::read(pair.git(1).join("HEAD.lock")).unwrap(),
            b"ref: refs/heads/other\n"
        );
        assert!(
            write_symbolic(&repo, "HEAD", "refs/heads/third", b"", || panic!(
                "lock must refuse before identity"
            ))
            .is_err()
        );
        assert!(Update::lock(&repo, "HEAD", Expected::Any).is_err());
        assert_eq!(before, pair.state(1));
        assert_eq!(
            fs::read(pair.git(1).join("HEAD")).unwrap(),
            b"ref: refs/heads/main\n"
        );
        resume_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(!pair.git(1).join("HEAD.lock").exists());
        assert_eq!(repo.resolve("HEAD").unwrap().to_string(), second);
        assert_eq!(repo.resolve("refs/heads/main").unwrap().to_string(), first);
    }
}

#[test]
fn identity_failure_obeys_named_and_associated_head_publication_boundaries() {
    use git_core::{refs::write_symbolic, repository::Repository, Error};
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for target_exists in [true, false] {
            let pair = Pair::new(hash);
            let [first, second] = pair.commits();
            pair.setup_ref("refs/heads/main", &first);
            if target_exists {
                pair.setup_ref("refs/heads/other", &second);
            }
            let repo = Repository::discover(&pair.roots[1]).unwrap();
            let logs = ["logs/HEAD", "logs/refs/heads/main"]
                .map(|name| fs::read(pair.git(1).join(name)).unwrap());
            assert!(write_symbolic(
                &repo,
                "refs/heads/main",
                "refs/heads/other",
                b"identity failure",
                || Err(Error::Invalid("missing identity".into()))
            )
            .is_err());
            assert_eq!(
                fs::read(pair.git(1).join("refs/heads/main")).unwrap(),
                if target_exists {
                    format!("{first}\n").into_bytes()
                } else {
                    b"ref: refs/heads/other\n".to_vec()
                }
            );
            for (name, before) in ["logs/HEAD", "logs/refs/heads/main"].into_iter().zip(logs) {
                assert_eq!(fs::read(pair.git(1).join(name)).unwrap(), before);
            }
            assert!(!pair.git(1).join("HEAD.lock").exists());
            assert!(!pair.git(1).join("refs/heads/main.lock").exists());
        }
    }
}

#[test]
fn packed_namespace_conflicts_preserve_existing_refs_and_release_owned_locks() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for (existing, source) in [
            ("refs/heads/packed", "refs/heads/packed/child"),
            ("refs/heads/packed/child", "refs/heads/packed"),
        ] {
            let pair = Pair::new(hash);
            let [first, _] = pair.commits();
            pair.setup_ref(existing, &first);
            for side in 0..2 {
                assert!(invoke(
                    &pair.reference,
                    &pair.roots[side],
                    &pair.home,
                    &["pack-refs", "--all"],
                    b"",
                    false
                )
                .unwrap()
                .status
                .success());
            }
            pair.compare(&["symbolic-ref", source, "refs/heads/unborn"], 1);
            assert_eq!(
                git_core::repository::Repository::discover(&pair.roots[1])
                    .unwrap()
                    .resolve(existing)
                    .unwrap()
                    .to_string(),
                first
            );
            assert!(!pair.git(1).join(format!("{source}.lock")).exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn symbolic_writes_refuse_source_symlinks_without_changing_the_outside_file() {
    use git_core::{refs::write_symbolic, repository::Repository};
    use std::os::unix::fs::symlink;
    let pair = Pair::new(HashKind::Sha1);
    let outside = pair._fixture.path.join("outside");
    fs::write(&outside, b"keep outside\n").unwrap();
    symlink(&outside, pair.git(1).join("refs/heads/alias")).unwrap();
    let repo = Repository::discover(&pair.roots[1]).unwrap();
    assert!(write_symbolic(
        &repo,
        "refs/heads/alias",
        "refs/heads/unborn",
        b"",
        || panic!("refuse path first")
    )
    .is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"keep outside\n");
    assert!(fs::symlink_metadata(pair.git(1).join("refs/heads/alias"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!pair.git(1).join("refs/heads/alias.lock").exists());
}

#[test]
fn symbolic_writes_cannot_publish_a_pointer_larger_than_the_read_bound() {
    use git_core::{refs::write_symbolic, repository::Repository};
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let pair = Pair::new(hash);
        let repo = Repository::discover(&pair.roots[1]).unwrap();
        let before = pair.state(1);
        let target = format!("refs/heads/{}a", "a/".repeat(2040));
        assert_eq!(target.len(), 4092);
        assert!(git_core::refs::valid_ref(&target));
        assert!(write_symbolic(&repo, "HEAD", &target, b"", || panic!(
            "oversized pointer requires no identity"
        ))
        .is_err());
        assert_eq!(before, pair.state(1));
    }
}

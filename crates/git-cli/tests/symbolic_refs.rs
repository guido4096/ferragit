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
    roots: [PathBuf; 2],
    home: PathBuf,
    reference: PathBuf,
    candidate: PathBuf,
}
impl Pair {
    fn new(hash: HashKind) -> Self {
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
            roots,
            home,
            reference,
            candidate,
        };
        for root in &pair.roots {
            let out = invoke(
                &pair.reference,
                root,
                &pair.home,
                &[
                    "init",
                    "-q",
                    "--template=",
                    "-b",
                    "main",
                    &format!("--object-format={}", hash.name()),
                ],
                b"",
                false,
            )
            .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        pair
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        for root in &self.roots {
            let path = root.join(".git").join(relative);
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
        walk(
            &self.roots[side],
            &self.roots[side].join(".git"),
            &mut result,
        );
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

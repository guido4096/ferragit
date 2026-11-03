// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! C Git produces the packed fixtures; rugit reads and continues them with an
//! empty PATH/exec path. No project repository is repacked by these tests.
use git_conformance::{executable, invoke, Fixture};
use git_core::{
    object::{HashKind, Object, ObjectId, ObjectKind},
    push,
    repository::Repository,
};
use std::{ffi::OsStr, fs, path::PathBuf, process::Output};

struct Harness {
    _fixture: Fixture,
    root: PathBuf,
    home: PathBuf,
    reference: PathBuf,
    candidate: PathBuf,
}

impl Harness {
    fn new(hash: HashKind) -> Self {
        let fixture = Fixture::new().unwrap();
        let root = fixture.path.join("repo");
        let home = fixture.path.join("home");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&home).unwrap();
        fs::write(home.join("empty-config"), b"").unwrap();
        let reference =
            executable(&std::env::var_os("RUGIT_REFERENCE").unwrap_or_else(|| "git".into()))
                .unwrap();
        let candidate = executable(OsStr::new(env!("CARGO_BIN_EXE_rugit"))).unwrap();
        let harness = Self {
            _fixture: fixture,
            root,
            home,
            reference,
            candidate,
        };
        harness.run(
            true,
            &[
                "init",
                "-q",
                "--template=",
                "-b",
                "main",
                &format!("--object-format={}", hash.name()),
            ],
            b"",
        );
        harness
    }
    fn run(&self, candidate: bool, args: &[&str], input: &[u8]) -> Output {
        self.run_at(candidate, &self.root, args, input)
    }
    fn run_at(
        &self,
        candidate: bool,
        cwd: &std::path::Path,
        args: &[&str],
        input: &[u8],
    ) -> Output {
        let output = invoke(
            if candidate {
                &self.candidate
            } else {
                &self.reference
            },
            cwd,
            &self.home,
            args,
            input,
            candidate,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "argv={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn compare(&self, args: &[&str]) {
        let reference = self.run(false, args, b"");
        let candidate = self.run(true, args, b"");
        assert!(
            reference.stdout == candidate.stdout,
            "stdout differs for {args:?}"
        );
        assert!(
            reference.stderr == candidate.stderr,
            "stderr differs for {args:?}"
        );
    }
    fn repo(&self) -> Repository {
        Repository::discover(&self.root).unwrap()
    }
}

fn exercise_real_packs(hash: HashKind, offset_delta: bool) {
    let harness = Harness::new(hash);
    fs::create_dir(harness.root.join("nested")).unwrap();
    fs::write(harness.root.join("nested/binary"), b"\0\xffbinary\r\n").unwrap();
    for revision in 0..8 {
        let text=(0..400).map(|row|format!("row{row:04}: stable payload {:016x} and more unchanged text for delta matching {}\n",row as u64*2654435761,if row==revision {"changed"}else{"original"})).collect::<String>();
        fs::write(harness.root.join("data.txt"), text).unwrap();
        harness.run(true, &["add", "-A"], b"");
        harness.run(
            true,
            &["commit", "-q", "-m", &format!("revision {revision}")],
            b"",
        );
    }
    let repo = harness.repo();
    let head = repo.resolve("HEAD").unwrap();
    let tag=repo.objects.write(&Object {kind:ObjectKind::Tag,data:format!("object {head}\ntype commit\ntag v1\ntagger Conformance Author <author@example.invalid> 1700000000 +0000\n\nFixture tag\n").into_bytes()}).unwrap();
    fs::write(repo.git_dir.join("refs/tags/v1"), format!("{tag}\n")).unwrap();
    let listing = harness.run(
        false,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ],
        b"",
    );
    let objects: Vec<(String, String)> = String::from_utf8(listing.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (id, kind) = line.split_once(' ').unwrap();
            (id.to_owned(), kind.to_owned())
        })
        .collect();
    for kind in ["blob", "tree", "commit", "tag"] {
        assert!(objects.iter().any(|(_, value)| value == kind));
    }
    let input = objects
        .iter()
        .map(|(id, _)| format!("{id}\n"))
        .collect::<String>();
    let delta_flag = if offset_delta {
        "--delta-base-offset"
    } else {
        "--no-delta-base-offset"
    };
    let packed = harness.run(
        false,
        &[
            "pack-objects",
            "--stdout",
            "--quiet",
            "--threads=1",
            "--window=50",
            "--depth=10",
            delta_flag,
        ],
        input.as_bytes(),
    );
    harness.run(
        false,
        &["index-pack", "--stdin", "--index-version=2"],
        &packed.stdout,
    );
    let directory = repo.objects.directory.join("pack");
    let index_path = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "idx"))
        .unwrap();
    let pack_path = index_path.with_extension("pack");
    let bytes = fs::read(&pack_path).unwrap();
    let verified = harness.run(
        false,
        &["verify-pack", "-v", index_path.to_str().unwrap()],
        b"",
    );
    let mut delta_id = None;
    for line in String::from_utf8(verified.stdout).unwrap().lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 7 || ObjectId::parse(hash, fields[0]).is_err() {
            continue;
        }
        let offset = fields[4].parse::<usize>().unwrap();
        assert_eq!(
            (bytes[offset] >> 4) & 7,
            if offset_delta { 6 } else { 7 },
            "fixture did not use the requested delta encoding"
        );
        delta_id = Some(fields[0].to_owned());
    }
    let delta_id = delta_id.expect("fixture must actually contain deltas");
    // Remove only this test fixture's loose objects; the complete object set is
    // already installed and independently verified by Git above.
    for entry in fs::read_dir(&repo.objects.directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_str().is_some_and(|name| {
            name.len() == 2 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            fs::remove_dir_all(entry.path()).unwrap();
        }
    }
    harness.run(false, &["pack-refs", "--all"], b"");
    assert!(!repo.git_dir.join("refs/heads/main").exists());
    assert!(!repo.git_dir.join("refs/tags/v1").exists());
    harness.compare(&["rev-parse", "HEAD"]);
    harness.compare(&["cat-file", "tag", "refs/tags/v1"]);
    assert!(harness
        .repo()
        .read_ref("refs/heads/missing", 0)
        .unwrap()
        .is_none());
    for (id, kind) in &objects {
        harness.compare(&["cat-file", kind, id]);
    }
    harness.compare(&["cat-file", "-t", &delta_id[..8]]);
    harness.compare(&["cat-file", "-s", &delta_id]);
    // A duplicate loose copy and two indexes naming the same ID must not cause
    // false abbreviation ambiguity.
    let object = harness
        .repo()
        .objects
        .read(&ObjectId::parse(hash, &delta_id).unwrap())
        .unwrap();
    harness.repo().objects.write(&object).unwrap();
    harness.compare(&["cat-file", "-t", &delta_id[..8]]);
    let fanout = repo.objects.directory.join(&delta_id[..2]);
    fs::remove_file(fanout.join(&delta_id[2..])).unwrap();
    // Keep the first object at offset 12 in the short table. Git rejects an
    // index with every entry escaped, even if --index-version=2,0 creates it.
    for version in ["1", "2,12"] {
        remove_fixture_index(&index_path);
        harness.run(
            false,
            &[
                "index-pack",
                &format!("--index-version={version}"),
                "-o",
                index_path.to_str().unwrap(),
                pack_path.to_str().unwrap(),
            ],
            b"",
        );
        harness.compare(&["cat-file", "blob", &delta_id]);
        harness.compare(&["cat-file", "commit", "HEAD"]);
    }
    // Use the Rust sender on a packed source and a genuine Git receive-pack peer.
    let receiver = harness._fixture.path.join("receiver");
    fs::create_dir(&receiver).unwrap();
    harness.run_at(
        false,
        &receiver,
        &[
            "init",
            "--bare",
            "-q",
            "--template=",
            &format!("--object-format={}", hash.name()),
        ],
        b"",
    );
    let advertisement = harness.run_at(
        false,
        &receiver,
        &["receive-pack", "--stateless-rpc", "--advertise-refs", "."],
        b"",
    );
    let mut wire = push::packet(b"# service=git-receive-pack\n").unwrap();
    wire.extend(b"0000");
    wire.extend(advertisement.stdout);
    let advertisement = push::Advertisement::parse(&wire, hash).unwrap();
    let request = push::request(&harness.repo(), &head, "refs/heads/main", &advertisement).unwrap();
    let status = harness.run_at(
        false,
        &receiver,
        &["receive-pack", "--stateless-rpc", "."],
        &request,
    );
    push::check_status(&status.stdout, "refs/heads/main").unwrap();
    harness.run_at(
        false,
        &receiver,
        &["fsck", "--strict", "--no-dangling"],
        b"",
    );
    // Another valid pack of the reachable objects exercises duplicate-ID lookup
    // across indexes without relying on nonstandard pack filenames.
    let second_pack = push::pack(&harness.repo(), &head, None).unwrap();
    harness.run(false, &["index-pack", "--stdin"], &second_pack);
    harness.compare(&["cat-file", "-t", &delta_id[..8]]);
    fs::write(
        harness.root.join("after-repack.txt"),
        b"new loose objects after packed history\n",
    )
    .unwrap();
    harness.run(true, &["add", "-A"], b"");
    harness.run(
        true,
        &["commit", "-q", "-m", "continue packed history"],
        b"",
    );
    assert!(repo.git_dir.join("refs/heads/main").exists());
    assert_ne!(harness.repo().resolve("HEAD").unwrap(), head);
    harness.compare(&["rev-parse", "HEAD"]);
    harness.compare(&["cat-file", "commit", "HEAD"]);
    harness.run(false, &["fsck", "--strict", "--no-dangling"], b"");
}

#[test]
fn reads_and_continues_git_packs_for_both_hashes_and_delta_encodings() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        for offset_delta in [false, true] {
            exercise_real_packs(hash, offset_delta);
        }
    }
}

fn remove_fixture_index(path: &std::path::Path) {
    #[cfg(windows)]
    {
        // Git can mark generated indexes read-only on Windows. Clearing that
        // attribute here changes only a disposable oracle fixture.
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }
    fs::remove_file(path).unwrap();
}

// Hand-built packs let the failure tests recompute outer checksums, ensuring
// validation reaches CRCs, delta bounds and object identity rather than merely
// rejecting every mutation at the first file checksum.
fn raw_entry(kind: u8, declared: u64, prefix: &[u8], data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::new();
    let mut size = declared;
    let mut byte = (kind << 4) | (size as u8 & 15);
    size >>= 4;
    if size != 0 {
        byte |= 128;
    }
    raw.push(byte);
    while size != 0 {
        let mut byte = (size & 127) as u8;
        size >>= 7;
        if size != 0 {
            byte |= 128;
        }
        raw.push(byte);
    }
    raw.extend(prefix);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    raw.extend(encoder.finish().unwrap());
    raw
}

fn object_id(hash: HashKind, data: &[u8]) -> ObjectId {
    hash.digest(
        &Object {
            kind: ObjectKind::Blob,
            data: data.to_vec(),
        }
        .encode(),
    )
    .unwrap()
}

fn install_pack(harness: &Harness, entries: Vec<(ObjectId, Vec<u8>)>) -> (PathBuf, PathBuf) {
    install_pack_version(harness, entries, 2)
}

fn install_pack_version(
    harness: &Harness,
    entries: Vec<(ObjectId, Vec<u8>)>,
    version: u32,
) -> (PathBuf, PathBuf) {
    let hash = harness.repo().objects.hash;
    let mut pack = b"PACK".to_vec();
    pack.extend(version.to_be_bytes());
    pack.extend((entries.len() as u32).to_be_bytes());
    let mut records = Vec::new();
    for (id, raw) in entries {
        let offset = pack.len() as u32;
        let crc = crc32fast::hash(&raw);
        pack.extend(raw);
        records.push((id, crc, offset));
    }
    let checksum = hash.digest(&pack).unwrap();
    pack.extend(checksum.bytes());
    let base = harness
        .repo()
        .objects
        .directory
        .join("pack")
        .join(format!("pack-{checksum}"));
    let pack_path = base.with_extension("pack");
    let index_path = base.with_extension("idx");
    fs::write(&pack_path, pack).unwrap();
    records.sort_by(|a, b| a.0.bytes().cmp(b.0.bytes()));
    let mut index = b"\xfftOc".to_vec();
    index.extend(2u32.to_be_bytes());
    for i in 0..256 {
        index.extend(
            (records
                .iter()
                .filter(|record| usize::from(record.0.bytes()[0]) <= i)
                .count() as u32)
                .to_be_bytes(),
        );
    }
    for (id, _, _) in &records {
        index.extend(id.bytes());
    }
    for (_, crc, _) in &records {
        index.extend(crc.to_be_bytes());
    }
    for (_, _, offset) in &records {
        index.extend(offset.to_be_bytes());
    }
    index.extend(checksum.bytes());
    let checksum = hash.digest(&index).unwrap();
    index.extend(checksum.bytes());
    fs::write(&index_path, index).unwrap();
    (pack_path, index_path)
}

fn resign_index(hash: HashKind, bytes: &mut [u8]) {
    let end = bytes.len() - hash.hex_len() / 2;
    let checksum = hash.digest(&bytes[..end]).unwrap();
    bytes[end..].copy_from_slice(checksum.bytes());
}

#[test]
fn version_three_packs_are_interchangeable_with_git() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let harness = Harness::new(hash);
        let id = object_id(hash, b"version three");
        install_pack_version(
            &harness,
            vec![(id.clone(), raw_entry(3, 13, &[], b"version three"))],
            3,
        );
        harness.compare(&["cat-file", "blob", &id.to_string()]);
    }
}

#[test]
fn malformed_packed_refs_prevent_ref_updates_and_release_locks() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let harness = Harness::new(hash);
        fs::write(harness.root.join("file"), b"staged data").unwrap();
        harness.run(true, &["add", "-A"], b"");
        let repo = harness.repo();
        let index = fs::read(repo.git_dir.join("index")).unwrap();
        let head = fs::read(repo.git_dir.join("HEAD")).unwrap();
        fs::write(repo.git_dir.join("packed-refs"), b"malformed record\n").unwrap();
        for candidate in [false, true] {
            let output = invoke(
                if candidate {
                    &harness.candidate
                } else {
                    &harness.reference
                },
                &harness.root,
                &harness.home,
                &["commit", "-q", "-m", "must fail"],
                b"",
                candidate,
            )
            .unwrap();
            assert!(
                !output.status.success(),
                "commit ignored corrupt packed refs"
            );
        }
        assert_eq!(fs::read(repo.git_dir.join("HEAD")).unwrap(), head);
        assert_eq!(fs::read(repo.git_dir.join("index")).unwrap(), index);
        assert!(!repo.git_dir.join("refs/heads/main").exists());
        assert!(!repo.git_dir.join("refs/heads/main.lock").exists());
        assert!(!repo.git_dir.join("index.lock").exists());
        fs::write(repo.git_dir.join("packed-refs"), b"").unwrap();
        harness.run(
            true,
            &["commit", "-q", "-m", "recover after metadata repair"],
            b"",
        );
        harness.compare(&["rev-parse", "HEAD"]);
    }
}

#[test]
fn pack_and_index_corruption_never_return_unverified_data() {
    for hash in [HashKind::Sha1, HashKind::Sha256] {
        let harness = Harness::new(hash);
        let id = object_id(hash, b"payload");
        let (pack_path, index_path) = install_pack(
            &harness,
            vec![(id.clone(), raw_entry(3, 7, &[], b"payload"))],
        );
        let original_pack = fs::read(&pack_path).unwrap();
        let original_index = fs::read(&index_path).unwrap();
        let mut bytes = original_index.clone();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&index_path, &bytes).unwrap();
        assert!(harness
            .repo()
            .objects
            .read(&id)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        let mut bytes = original_index.clone();
        bytes[8 + 1024 + hash.hex_len() / 2] ^= 1;
        resign_index(hash, &mut bytes);
        fs::write(&index_path, &bytes).unwrap();
        assert!(harness
            .repo()
            .objects
            .read(&id)
            .unwrap_err()
            .to_string()
            .contains("CRC"));
        let mut bytes = original_index.clone();
        let offset = 8 + 1024 + hash.hex_len() / 2 + 4;
        bytes[offset..offset + 4].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
        resign_index(hash, &mut bytes);
        fs::write(&index_path, &bytes).unwrap();
        assert!(harness
            .repo()
            .objects
            .read(&id)
            .unwrap_err()
            .to_string()
            .contains("offset"));
        fs::write(&index_path, &original_index).unwrap();
        let mut bytes = original_pack.clone();
        bytes[12] ^= 1;
        fs::write(&pack_path, &bytes).unwrap();
        assert!(harness
            .repo()
            .objects
            .read(&id)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        fs::write(&pack_path, &original_pack[..original_pack.len() - 1]).unwrap();
        assert!(harness.repo().objects.read(&id).is_err());
        fs::write(&pack_path, &original_pack).unwrap();
        assert_eq!(harness.repo().objects.read(&id).unwrap().data, b"payload");
    }
}

#[test]
fn valid_checksums_do_not_hide_wrong_ids_or_bad_zlib_boundaries() {
    for mode in 0..3 {
        let harness = Harness::new(HashKind::Sha1);
        let id = object_id(
            HashKind::Sha1,
            if mode == 0 { b"other" } else { b"payload" },
        );
        let mut entry = raw_entry(3, if mode == 2 { 2 } else { 7 }, &[], b"payload");
        if mode == 1 {
            entry.extend(b"unindexed trailing bytes");
        }
        install_pack(&harness, vec![(id.clone(), entry)]);
        let error = harness.repo().objects.read(&id).unwrap_err().to_string();
        assert!(
            if mode == 0 {
                error.contains("object ID mismatch")
            } else if mode == 1 {
                error.contains("boundary")
            } else {
                error.contains("exceeds")
            },
            "{error}"
        );
    }
}

#[test]
fn malformed_deltas_and_declared_size_bombs_fail_before_excessive_work() {
    for mode in 0..5 {
        let harness = Harness::new(HashKind::Sha256);
        let id = object_id(HashKind::Sha256, b"target");
        let raw = match mode {
            0 => raw_entry(3, 64 * 1024 * 1024 + 1, &[], b""),
            1 => raw_entry(6, 4, &[0], &[1, 1, 0x90, 1]),
            2 => raw_entry(6, 4, &[1], &[1, 1, 0x90, 1]),
            3 => raw_entry(7, 4, &[1, 2], &[1, 1, 0x90, 1]),
            _ => raw_entry(7, 7, id.bytes(), &[1, 0x80, 0x80, 0x80, 0x80, 1, 0]),
        };
        install_pack(&harness, vec![(id.clone(), raw)]);
        assert!(harness.repo().objects.read(&id).is_err());
    }
}

#[test]
fn cyclic_and_overdeep_ref_deltas_are_bounded() {
    for cross_pack in [false, true] {
        let harness = Harness::new(HashKind::Sha1);
        let a = object_id(HashKind::Sha1, b"a");
        let b = object_id(HashKind::Sha1, b"b");
        let first = (
            a.clone(),
            raw_entry(
                7,
                4,
                if cross_pack { b.bytes() } else { a.bytes() },
                &[1, 1, 0x90, 1],
            ),
        );
        install_pack(&harness, vec![first]);
        if cross_pack {
            install_pack(
                &harness,
                vec![(b, raw_entry(7, 4, a.bytes(), &[1, 1, 0x90, 1]))],
            );
        }
        assert!(harness
            .repo()
            .objects
            .read(&a)
            .unwrap_err()
            .to_string()
            .contains("cyclic"));
    }
    let harness = Harness::new(HashKind::Sha256);
    let mut bytes = vec![b'x'];
    let mut id = object_id(HashKind::Sha256, &bytes);
    let mut entries = vec![(id.clone(), raw_entry(3, 1, &[], &bytes))];
    let mut allowed = id.clone();
    for length in 1u8..=65 {
        bytes.push(b'x');
        let next = object_id(HashKind::Sha256, &bytes);
        let delta = [length, length + 1, 0x90, length, 1, b'x'];
        entries.push((
            next.clone(),
            raw_entry(7, delta.len() as u64, id.bytes(), &delta),
        ));
        if length == 64 {
            allowed = next.clone();
        }
        id = next;
    }
    install_pack(&harness, entries);
    assert!(harness
        .repo()
        .objects
        .read(&id)
        .unwrap_err()
        .to_string()
        .contains("64 links"));
    assert_eq!(
        harness.repo().objects.read(&allowed).unwrap().data.len(),
        65
    );
}

#[test]
fn ref_delta_bases_can_be_resolved_from_loose_storage() {
    let harness = Harness::new(HashKind::Sha1);
    let repo = harness.repo();
    let base = repo
        .objects
        .write(&Object {
            kind: ObjectKind::Blob,
            data: b"hello".to_vec(),
        })
        .unwrap();
    let id = object_id(HashKind::Sha1, b"hello!");
    install_pack(
        &harness,
        vec![(
            id.clone(),
            raw_entry(7, 6, base.bytes(), &[5, 6, 0x90, 5, 1, b'!']),
        )],
    );
    assert_eq!(repo.objects.read(&id).unwrap().data, b"hello!");
}

#[test]
fn packed_and_loose_prefix_collisions_are_reported_as_ambiguous() {
    use std::collections::BTreeMap;
    let harness = Harness::new(HashKind::Sha1);
    let mut seen = BTreeMap::new();
    let (prefix, first, first_data, second_data) = loop {
        let n = seen.len();
        assert!(n <= 65536);
        let data = format!("prefix collision fixture {n}").into_bytes();
        let id = object_id(HashKind::Sha1, &data);
        let prefix = id.to_string()[..4].to_owned();
        if let Some((previous, bytes)) = seen.remove(&prefix) {
            break (prefix, previous, bytes, data);
        }
        seen.insert(prefix, (id, data));
    };
    install_pack(
        &harness,
        vec![(
            first,
            raw_entry(3, first_data.len() as u64, &[], &first_data),
        )],
    );
    harness
        .repo()
        .objects
        .write(&Object {
            kind: ObjectKind::Blob,
            data: second_data,
        })
        .unwrap();
    assert!(harness
        .repo()
        .objects
        .resolve_hex(&prefix)
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
}

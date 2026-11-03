// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

use git_conformance::{executable, invoke, Fixture};
use git_core::{
    object::{Object, ObjectKind},
    repository::Repository,
};
use std::{fs, path::Path, thread};

fn setup() -> (Fixture, std::path::PathBuf) {
    let fixture = Fixture::new().unwrap();
    fs::write(fixture.path.join("empty-config"), b"").unwrap();
    let candidate = executable(Path::new(env!("CARGO_BIN_EXE_ferragit")).as_os_str()).unwrap();
    let output = invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["init", "-q", "--template=", "-b", "main"],
        b"",
        true,
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    (fixture, candidate)
}

#[test]
fn stale_index_lock_preserves_the_previous_index() {
    let (fixture, candidate) = setup();
    fs::write(fixture.path.join("file"), b"old").unwrap();
    assert!(invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["add", "-A"],
        b"",
        true
    )
    .unwrap()
    .status
    .success());
    let index = fixture.path.join(".git/index");
    let original = fs::read(&index).unwrap();
    fs::write(fixture.path.join("file"), b"new").unwrap();
    fs::write(fixture.path.join(".git/index.lock"), b"other writer").unwrap();
    assert!(!invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["add", "-A"],
        b"",
        true
    )
    .unwrap()
    .status
    .success());
    assert_eq!(fs::read(&index).unwrap(), original);
    assert_eq!(
        fs::read(fixture.path.join(".git/index.lock")).unwrap(),
        b"other writer"
    );
}

#[test]
fn stale_ref_lock_does_not_create_a_commit_or_remove_the_other_lock() {
    let (fixture, candidate) = setup();
    fs::write(fixture.path.join("file"), b"content").unwrap();
    assert!(invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["add", "-A"],
        b"",
        true
    )
    .unwrap()
    .status
    .success());
    fs::write(
        fixture.path.join(".git/refs/heads/main.lock"),
        b"another writer",
    )
    .unwrap();
    assert!(!invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["commit", "-q", "-m", "must fail"],
        b"",
        true
    )
    .unwrap()
    .status
    .success());
    assert!(!fixture.path.join(".git/refs/heads/main").exists());
    assert!(!fixture.path.join(".git/index.lock").exists());
    assert_eq!(
        fs::read(fixture.path.join(".git/refs/heads/main.lock")).unwrap(),
        b"another writer"
    );
}

#[test]
fn unknown_repository_extensions_prevent_object_writes() {
    let (fixture, candidate) = setup();
    let config = fixture.path.join(".git/config");
    fs::write(&config,b"[core]\nrepositoryformatversion=1\nbare=false\n[extensions]\nfutureRequiredFeature=true\n").unwrap();
    let original = fs::read(&config).unwrap();
    assert!(!invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["hash-object", "-w", "--stdin"],
        b"must not store",
        true
    )
    .unwrap()
    .status
    .success());
    assert_eq!(fs::read(config).unwrap(), original);
    assert_eq!(
        fs::read_dir(fixture.path.join(".git/objects"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn corrupt_existing_object_is_not_overwritten_or_reported_as_success() {
    let (fixture, candidate) = setup();
    let repo = Repository::discover(&fixture.path).unwrap();
    let object = Object {
        kind: ObjectKind::Blob,
        data: b"same name".to_vec(),
    };
    let id = repo.objects.write(&object).unwrap().to_string();
    let path = repo.git_dir.join("objects").join(&id[..2]).join(&id[2..]);
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"corrupt existing object").unwrap();
    assert!(!invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["hash-object", "-w", "--stdin"],
        &object.data,
        true
    )
    .unwrap()
    .status
    .success());
    assert_eq!(fs::read(&path).unwrap(), b"corrupt existing object");
    assert!(!fs::read_dir(path.parent().unwrap())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("tmp_obj_")));
}

#[test]
fn concurrent_object_writers_produce_one_valid_object_without_temporary_files() {
    let (fixture, candidate) = setup();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let cwd = fixture.path.clone();
        let binary = candidate.clone();
        handles.push(thread::spawn(move || {
            invoke(
                &binary,
                &cwd,
                &cwd,
                &["hash-object", "-w", "--stdin"],
                b"concurrent content",
                true,
            )
            .unwrap()
        }));
    }
    let outputs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for output in &outputs {
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, outputs[0].stdout);
    }
    let repo = Repository::discover(&fixture.path).unwrap();
    let name = String::from_utf8(outputs[0].stdout.clone()).unwrap();
    assert_eq!(
        repo.objects
            .read(&repo.resolve(name.trim()).unwrap())
            .unwrap()
            .data,
        b"concurrent content"
    );
    for directory in fs::read_dir(repo.objects.directory).unwrap() {
        let directory = directory.unwrap();
        if directory.file_name().len() == 2 {
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }
}

#[test]
fn corrupted_index_does_not_replace_the_index_or_leave_a_lock() {
    let (fixture, candidate) = setup();
    let path = fixture.path.join(".git/index");
    fs::write(&path, b"bad index").unwrap();
    assert!(!invoke(
        &candidate,
        &fixture.path,
        &fixture.path,
        &["add", "-A"],
        b"",
        true
    )
    .unwrap()
    .status
    .success());
    assert_eq!(fs::read(path).unwrap(), b"bad index");
    assert!(!fixture.path.join(".git/index.lock").exists());
}

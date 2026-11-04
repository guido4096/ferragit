// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Tree construction and simple single-parent commits, using the ordinary index.
use crate::{
    identity::Identity,
    index::{self, Entry},
    object::{Object, ObjectId, ObjectKind},
    refs::{self, Expected, Update},
    repository::Repository,
    Error, Result,
};
use git_platform::LockFile;
use std::collections::BTreeMap;

enum Node {
    File(Entry),
    Directory(BTreeMap<Vec<u8>, Node>),
}

pub fn write_tree(repo: &Repository) -> Result<ObjectId> {
    let mut root = BTreeMap::new();
    for entry in index::read(repo)? {
        let components: Vec<Vec<u8>> = entry
            .path
            .split(|b| *b == b'/')
            .map(<[u8]>::to_vec)
            .collect();
        insert(&mut root, &components, entry)?;
    }
    store_tree(repo, root)
}

fn insert(directory: &mut BTreeMap<Vec<u8>, Node>, path: &[Vec<u8>], entry: Entry) -> Result<()> {
    if path.len() == 1 {
        if directory
            .insert(path[0].clone(), Node::File(entry))
            .is_some()
        {
            return Err(Error::Invalid("tree path conflict".into()));
        }
        return Ok(());
    }
    let node = directory
        .entry(path[0].clone())
        .or_insert_with(|| Node::Directory(BTreeMap::new()));
    match node {
        Node::Directory(children) => insert(children, &path[1..], entry),
        Node::File(_) => Err(Error::Invalid("tree file/directory conflict".into())),
    }
}

fn store_tree(repo: &Repository, directory: BTreeMap<Vec<u8>, Node>) -> Result<ObjectId> {
    let mut entries = Vec::new();
    for (name, node) in directory {
        let (mode, id, is_dir) = match node {
            Node::File(entry) => (entry.mode(), entry.id, false),
            Node::Directory(children) => (0o40000, store_tree(repo, children)?, true),
        };
        // Git compares directory names as though they end with '/', not NUL.
        let mut sort_key = name.clone();
        if is_dir {
            sort_key.push(b'/');
        }
        entries.push((sort_key, name, mode, id));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut bytes = Vec::new();
    for (_, name, mode, id) in entries {
        bytes.extend_from_slice(format!("{mode:o} ").as_bytes());
        bytes.extend_from_slice(&name);
        bytes.push(0);
        bytes.extend_from_slice(id.bytes());
    }
    repo.objects.write(&Object {
        kind: ObjectKind::Tree,
        data: bytes,
    })
}

pub fn create(
    repo: &Repository,
    message: &[u8],
    author: &Identity,
    committer: &Identity,
) -> Result<ObjectId> {
    author.validate()?;
    committer.validate()?;
    if message.is_empty() || message.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::Invalid("empty commit message".into()));
    }
    if repo.config.boolean("commit.gpgsign", false)? {
        return Err(Error::Unsupported("commit signing".into()));
    }
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        if repo.git_dir.join(marker).exists() {
            return Err(Error::Unsupported(
                "commit during an in-progress operation".into(),
            ));
        }
    }
    let hooks = refs::hooks_directory(repo);
    for hook in [
        "pre-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
        "reference-transaction",
    ] {
        if hooks.join(hook).exists() {
            return Err(Error::Unsupported(format!("commit hook {hook}")));
        }
    }
    let update = Update::lock(repo, "HEAD", Expected::Any)?;
    // Check attached HEAD while its lock is held.
    repo.head_target()?;
    // Holding the index lock prevents concurrent staging during snapshot reads.
    let _index_lock = LockFile::acquire(&repo.git_dir.join("index"))?;
    let parent = update.old().cloned();
    if let Some(parent) = &parent {
        if repo.objects.read(parent)?.kind != ObjectKind::Commit {
            return Err(Error::Invalid("HEAD does not name a commit".into()));
        }
    }
    let tree = write_tree(repo)?;
    let mut data = format!("tree {tree}\n").into_bytes();
    if let Some(parent) = &parent {
        data.extend_from_slice(format!("parent {parent}\n").as_bytes());
    }
    data.extend_from_slice(
        format!(
            "author {}\ncommitter {}\n\n",
            author.line(),
            committer.line()
        )
        .as_bytes(),
    );
    data.extend_from_slice(message);
    if !data.ends_with(b"\n") {
        data.push(b'\n');
    }
    let id = repo.objects.write(&Object {
        kind: ObjectKind::Commit,
        data,
    })?;
    let mut reason = if parent.is_none() {
        b"commit (initial): ".to_vec()
    } else {
        b"commit: ".to_vec()
    };
    reason.extend(
        message
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    );
    update.finish(&id, || Ok(committer.clone()), &reason)?;
    Ok(id)
}

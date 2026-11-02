// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Tree construction and simple single-parent commits, using the ordinary index.
use crate::{
    index::{self, Entry},
    object::{Object, ObjectId, ObjectKind},
    repository::Repository,
    Error, Result,
};
use git_platform::LockFile;
use std::{collections::BTreeMap, fs};

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

pub struct Identity {
    pub name: String,
    pub email: String,
    pub date: String,
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.email.is_empty()
            || self.name.contains(['\n', '\r', '\0', '<', '>'])
            || self.email.contains(['\n', '\r', '\0', '<', '>'])
        {
            return Err(Error::Invalid("invalid author/committer identity".into()));
        }
        let Some((seconds, zone)) = self.date.split_once(' ') else {
            return Err(Error::Unsupported(
                "date format; use '<unix-seconds> +HHMM'".into(),
            ));
        };
        if seconds.parse::<u64>().is_err()
            || zone.len() != 5
            || !matches!(zone.as_bytes()[0], b'+' | b'-')
            || !zone.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(Error::Invalid("invalid identity date".into()));
        }
        Ok(())
    }
    fn line(&self) -> String {
        format!("{} <{}> {}", self.name, self.email, self.date)
    }
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
    let hooks = repo
        .config
        .get("core.hookspath")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| repo.git_dir.join("hooks"));
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
    let target = repo.head_target()?;
    let ref_path = repo.git_dir.join(&target);
    fs::create_dir_all(ref_path.parent().unwrap())?;
    let lock = LockFile::acquire(&ref_path)?;
    // Holding the index lock prevents concurrent staging while we read the snapshot.
    let _index_lock = LockFile::acquire(&repo.git_dir.join("index"))?;
    let parent = repo.read_ref(&target, 0)?;
    if let Some(parent) = &parent {
        if repo.objects.read(parent)?.kind != ObjectKind::Commit {
            return Err(Error::Invalid("HEAD does not name a commit".into()));
        }
    }
    let tree = write_tree(repo)?;
    let mut data = format!("tree {tree}\n").into_bytes();
    if let Some(parent) = parent {
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
    lock.commit(&ref_path, format!("{id}\n").as_bytes())?;
    // Reflog transactions are not implemented yet; this difference is explicit
    // in compat/status.json rather than hidden behind the commit milestone.
    Ok(id)
}

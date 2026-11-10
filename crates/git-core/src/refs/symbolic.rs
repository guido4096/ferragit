// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Bounded symbolic-reference inspection, separate from terminal-ID updates.
use super::{hooks_directory, packed, reflog, safe_path, validate, MAX_REF_BYTES};
use crate::identity::Identity;
use crate::{object::ObjectId, repository::Repository, store::read_bounded, Error, Result};
use git_platform::LockFile;
use std::{fs, io};

// refs/refs-internal.h in the pinned reference counts the terminal lookup too.
const MAX_DEPTH: usize = 5;

pub enum SymbolicRead {
    Target(String),
    NotSymbolic,
    Unresolved,
}

enum Raw {
    Symbolic(String),
    Direct(ObjectId),
    Missing,
    Broken,
}

fn whitespace(byte: &u8) -> bool {
    // The reference excludes form feed and vertical tab from whitespace.
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn raw(repo: &Repository, name: &str) -> Result<Raw> {
    let traversable = match safe_path(&repo.git_dir, name) {
        Ok(()) => true,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotADirectory => false,
        Err(error) => return Err(error),
    };
    let path = repo.git_dir.join(name);
    let file = if !traversable || path.is_dir() {
        None
    } else {
        match fs::File::open(&path) {
            Ok(file) => Some(file),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }
            Err(error) => return Err(error.into()),
        }
    };
    let Some(file) = file else {
        safe_path(&repo.git_dir, "packed-refs")?;
        return Ok(
            match packed::lookup(&repo.git_dir.join("packed-refs"), repo.objects.hash, name)? {
                Some(id) => Raw::Direct(id),
                None => Raw::Missing,
            },
        );
    };
    let bytes = read_bounded(file, MAX_REF_BYTES)?;
    // Whole-file whitespace trimming precedes C-string truncation at NUL.
    let end = bytes
        .iter()
        .rposition(|byte| !whitespace(byte))
        .map_or(0, |index| index + 1);
    let bytes = bytes[..end]
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default();
    if let Some(target) = bytes.strip_prefix(b"ref:") {
        let start = target
            .iter()
            .position(|byte| !whitespace(byte))
            .unwrap_or(target.len());
        return Ok(Raw::Symbolic(
            std::str::from_utf8(&target[start..])
                .map_err(|_| Error::Unsupported("non-UTF-8 symbolic reference target".into()))?
                .to_owned(),
        ));
    }
    let width = repo.objects.hash.hex_len();
    if bytes.len() < width
        || !bytes[..width].iter().all(u8::is_ascii_hexdigit)
        || bytes.get(width).is_some_and(|byte| !whitespace(byte))
    {
        return Ok(Raw::Broken);
    }
    let text = std::str::from_utf8(&bytes[..width]).expect("validated hex is ASCII");
    Ok(Raw::Direct(ObjectId::parse(repo.objects.hash, text)?))
}

struct Inspection {
    name: String,
    value: Option<ObjectId>,
    symbolic: bool,
}

fn inspect(repo: &Repository, name: &str, recurse: bool) -> Result<Option<Inspection>> {
    let mut current = name.to_owned();
    let mut symbolic = false;
    for _ in 0..MAX_DEPTH {
        if validate(&current).is_err() {
            return Ok(None);
        }
        let value = match raw(repo, &current) {
            Ok(value) => value,
            Err(Error::Io(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        match value {
            Raw::Symbolic(target) => {
                symbolic = true;
                current = target;
                if !recurse {
                    return Ok(Some(Inspection {
                        name: current,
                        value: None,
                        symbolic,
                    }));
                }
            }
            Raw::Direct(value) => {
                return Ok(Some(Inspection {
                    name: current,
                    value: Some(value),
                    symbolic,
                }))
            }
            Raw::Missing => {
                return Ok(Some(Inspection {
                    name: current,
                    value: None,
                    symbolic,
                }))
            }
            Raw::Broken => return Ok(None),
        }
    }
    Ok(None)
}

pub fn read_symbolic(repo: &Repository, name: &str, recurse: bool) -> Result<SymbolicRead> {
    Ok(match inspect(repo, name, recurse)? {
        Some(Inspection {
            name,
            symbolic: true,
            ..
        }) => SymbolicRead::Target(name),
        Some(_) => SymbolicRead::NotSymbolic,
        None => SymbolicRead::Unresolved,
    })
}

fn head_points_to(repo: &Repository, name: &str) -> Result<bool> {
    Ok(matches!(raw(repo, "HEAD")?, Raw::Symbolic(target) if target == name))
}

pub fn write_symbolic(
    repo: &Repository,
    name: &str,
    target: &str,
    reason: &[u8],
    identity: impl FnOnce() -> Result<Identity>,
) -> Result<()> {
    validate(name)?;
    validate(target)?;
    if !target.starts_with("refs/") {
        return Err(Error::Unsupported("symbolic targets outside refs/".into()));
    }
    let contents = format!("ref: {target}\n");
    if contents.len() as u64 > MAX_REF_BYTES {
        return Err(Error::Unsupported(
            "symbolic reference exceeds 4 KiB".into(),
        ));
    }
    if hooks_directory(repo).join("reference-transaction").exists() {
        return Err(Error::Unsupported("reference-transaction hook".into()));
    }
    safe_path(&repo.git_dir, name)?;
    safe_path(&repo.git_dir, "packed-refs")?;
    // Refuse known packed namespace conflicts before creating loose directories.
    // The second check under lock still validates the publication state.
    packed::check_name_available(&repo.git_dir.join("packed-refs"), repo.objects.hash, name)?;
    let extra_head = name != "HEAD" && head_points_to(repo, name)?;
    let mut names = vec![name];
    if extra_head {
        names.push("HEAD");
    }
    names.sort_unstable();
    let mut locks = Vec::new();
    for name in names {
        safe_path(&repo.git_dir, name)?;
        let path = repo.git_dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        locks.push((name, LockFile::acquire(&path)?));
    }
    if extra_head && !head_points_to(repo, name)? {
        return Err(Error::Invalid("HEAD has been racily updated".into()));
    }
    packed::check_name_available(&repo.git_dir.join("packed-refs"), repo.objects.hash, name)?;
    let path = repo.git_dir.join(name);
    if path.is_dir() && fs::read_dir(&path)?.next().is_some() {
        return Err(Error::Unsupported(
            "nonempty directory at symbolic reference path".into(),
        ));
    }
    // Resolve before publishing: even creating a self-cycle can have a current
    // direct value. Missing/broken destinations omit the named reflog append.
    let old = match raw(repo, name)? {
        Raw::Direct(value) => Some(value),
        Raw::Symbolic(referent) => inspect(repo, &referent, true)?.and_then(|entry| entry.value),
        Raw::Missing | Raw::Broken => None,
    };
    let index = locks
        .iter()
        .position(|(locked, _)| *locked == name)
        .expect("named ref lock is owned");
    let (_, mut lock) = locks.remove(index);
    lock.prepare(contents.as_bytes())?;
    let new = inspect(repo, target, true)?.and_then(|entry| entry.value);
    let named_log = new.is_some() && reflog::enabled(repo, name)?;
    let head_log = extra_head && reflog::enabled(repo, "HEAD")?;
    let mut producer = Some(identity);
    let mut committer = None;
    if named_log {
        let who = producer.take().expect("identity is evaluated once")()?;
        let record = reflog::record(
            repo,
            old.as_ref(),
            new.as_ref().expect("named log has a new ID"),
            &who,
            reason,
        )?;
        reflog::append(repo, name, &record)?;
        committer = Some(who);
    }
    if path.is_dir() {
        fs::remove_dir(&path)?;
    }
    lock.publish(&path)?;
    // The reference's split HEAD update has no new_target and carries zero as
    // its new object ID. It is appended after publishing the named pointer.
    if head_log {
        let who = match committer {
            Some(who) => who,
            None => producer.take().expect("identity is evaluated once")()?,
        };
        let zero = ObjectId::from_bytes(
            repo.objects.hash,
            &[0; 32][..repo.objects.hash.hex_len() / 2],
        )?;
        let record = reflog::record(repo, old.as_ref(), &zero, &who, reason)?;
        reflog::append(repo, "HEAD", &record)?;
    }
    Ok(())
}

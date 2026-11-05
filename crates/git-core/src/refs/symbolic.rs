// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Bounded symbolic-reference inspection, separate from terminal-ID updates.
use super::{packed, safe_path, validate, MAX_REF_BYTES};
use crate::{object::ObjectId, repository::Repository, store::read_bounded, Error, Result};
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
    safe_path(&repo.git_dir, name)?;
    let path = repo.git_dir.join(name);
    let file = if path.is_dir() {
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
                        symbolic,
                    }));
                }
            }
            Raw::Direct(_value) => {
                return Ok(Some(Inspection {
                    name: current,
                    symbolic,
                }))
            }
            Raw::Missing => {
                return Ok(Some(Inspection {
                    name: current,
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

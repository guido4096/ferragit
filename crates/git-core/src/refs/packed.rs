// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Read-only packed-ref fallback. Loose refs are resolved by the caller first.
use super::valid_ref;
use crate::{
    object::{HashKind, ObjectId},
    store::read_bounded,
    Error, Result,
};
use std::{collections::BTreeMap, fs, io, path::Path};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REFS: usize = 100_000;

pub(super) fn lookup(path: &Path, hash: HashKind, reference: &str) -> Result<Option<ObjectId>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let bytes = read_bounded(file, MAX_BYTES)?;
    Ok(parse(hash, &bytes)?.remove(reference))
}

/// Rewrite only the canonical files-backend format emitted by pack-refs.
/// Unrelated records and their peeled IDs remain byte-for-byte intact.
pub(super) fn without(path: &Path, hash: HashKind, reference: &str) -> Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let bytes = read_bounded(file, MAX_BYTES)?;
    if !parse(hash, &bytes)?.contains_key(reference) {
        return Ok(None);
    }
    let text = std::str::from_utf8(&bytes).expect("packed parser validated UTF-8");
    let header = "# pack-refs with: peeled fully-peeled sorted \n";
    let Some(records) = text.strip_prefix(header) else {
        return Err(Error::Unsupported(
            "noncanonical packed-refs rewrite header".into(),
        ));
    };
    if records.lines().any(|line| line.starts_with('#')) {
        return Err(Error::Unsupported(
            "comments in packed-refs rewrite records".into(),
        ));
    }
    let mut result = header.as_bytes().to_vec();
    let mut omit = false;
    for line in records.split_inclusive('\n') {
        if !line.starts_with('^') {
            omit = line
                .trim_end_matches('\n')
                .split_once(' ')
                .is_some_and(|(_, name)| name == reference);
        }
        if !omit {
            result.extend_from_slice(line.as_bytes());
        }
    }
    Ok(Some(result))
}

fn invalid(message: &str) -> Error {
    Error::Invalid(format!("corrupt packed-refs: {message}"))
}

fn parse(hash: HashKind, bytes: &[u8]) -> Result<BTreeMap<String, ObjectId>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Unsupported("non-UTF-8 packed reference names".into()))?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(invalid("unterminated record"));
    }
    let mut refs = BTreeMap::new();
    let mut previous: Option<&str> = None;
    let mut peeled = false;
    let mut sorted = false;
    for line in text.split_terminator('\n') {
        if let Some(features) = line.strip_prefix("# pack-refs with: ") {
            if !refs.is_empty() {
                return Err(invalid("header after records"));
            }
            sorted = features
                .split_whitespace()
                .any(|feature| feature == "sorted");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(id) = line.strip_prefix('^') {
            if previous.is_none() || peeled {
                return Err(invalid("orphan or duplicate peeled ID"));
            }
            let id = ObjectId::parse(hash, id)?;
            if id.bytes().iter().all(|byte| *byte == 0) {
                return Err(invalid("null peeled object ID"));
            }
            peeled = true;
            continue;
        }
        let (id, name) = line
            .split_once(' ')
            .ok_or_else(|| invalid("malformed record"))?;
        if !valid_ref(name) {
            return Err(invalid("invalid reference name"));
        }
        if sorted && previous.is_some_and(|previous| previous >= name) {
            return Err(invalid("sorted header disagrees with records"));
        }
        let id = ObjectId::parse(hash, id)?;
        if id.bytes().iter().all(|byte| *byte == 0) {
            return Err(invalid("null object ID"));
        }
        if refs.insert(name.to_owned(), id).is_some() {
            return Err(invalid("duplicate reference name"));
        }
        if refs.len() > MAX_REFS {
            return Err(Error::Unsupported("more than 100,000 packed refs".into()));
        }
        previous = Some(name);
        peeled = false;
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_hashes_headers_and_peeled_tags() {
        for hash in [HashKind::Sha1, HashKind::Sha256] {
            let id = hash.digest(b"fixture").unwrap();
            let text=format!("# pack-refs with: peeled fully-peeled sorted \n{id} refs/heads/main\n{id} refs/tags/v1\n^{id}\n");
            let refs = parse(hash, text.as_bytes()).unwrap();
            assert_eq!(refs["refs/heads/main"], id);
            assert_eq!(refs.len(), 2);
        }
    }
    #[test]
    fn rejects_malformed_records_and_unsafe_names() {
        let id = "a".repeat(40);
        for text in [
            format!("^{id}\n"),
            format!("{id} refs/heads/../config\n"),
            format!("{id} refs/heads/a\n{id} refs/heads/a\n"),
            format!("{id} refs/tags/v1\n^{id}\n^{id}\n"),
            format!("# pack-refs with: sorted \n{id} refs/heads/z\n{id} refs/heads/a\n"),
            format!("{id} refs/heads/a"),
            format!("{id} refs/heads/a\r\n"),
            "bad refs/heads/a\n".into(),
            format!("{} refs/heads/a\n", "0".repeat(40)),
            format!("{id} refs/tags/a\n^{}\n", "0".repeat(40)),
        ] {
            assert!(
                parse(HashKind::Sha1, text.as_bytes()).is_err(),
                "accepted {text:?}"
            );
        }
    }
}

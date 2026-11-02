// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! The version-2 index, including stat fields and checksum. Only normal stage-0
//! entries are writable yet; unsupported extensions/stages fail before replacement.
use crate::{
    object::{HashKind, Object, ObjectId, ObjectKind},
    repository::Repository,
    store::{read_bounded, MAX_OBJECT_BYTES},
    Error, Result,
};
use git_platform::{path_bytes, LockFile};
use std::{collections::BTreeMap, fs, io};

#[derive(Debug, Clone)]
pub struct Entry {
    pub stat: [u32; 10],
    pub id: ObjectId,
    pub path: Vec<u8>,
}

impl Entry {
    pub fn mode(&self) -> u32 {
        self.stat[6]
    }
}

pub fn read(repo: &Repository) -> Result<Vec<Entry>> {
    let data = match fs::read(repo.git_dir.join("index")) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    decode(repo.objects.hash, &data)
}

pub fn decode(hash: HashKind, data: &[u8]) -> Result<Vec<Entry>> {
    let width = hash.hex_len() / 2;
    if data.len() < 12 + width || &data[..4] != b"DIRC" {
        return Err(Error::Invalid("invalid index header".into()));
    }
    let end = data.len() - width;
    if hash.digest(&data[..end])?.bytes() != &data[end..] {
        return Err(Error::Invalid("index checksum mismatch".into()));
    }
    if u32::from_be_bytes(data[4..8].try_into().unwrap()) != 2 {
        return Err(Error::Unsupported("index versions other than 2".into()));
    }
    let count = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
    let mut offset = 12;
    let mut entries: Vec<Entry> = Vec::new();
    for _ in 0..count {
        let start = offset;
        if end.saturating_sub(offset) < 40 + width + 3 {
            return Err(Error::Invalid("truncated index entry".into()));
        }
        let mut stat = [0; 10];
        for field in &mut stat {
            *field = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
        }
        let hex = data[offset..offset + width]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let id = ObjectId::parse(hash, &hex)?;
        offset += width;
        let flags = u16::from_be_bytes(data[offset..offset + 2].try_into().unwrap());
        offset += 2;
        if flags & 0xf000 != 0 {
            return Err(Error::Unsupported("index stages or flags".into()));
        }
        let length = data[offset..end]
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| Error::Invalid("unterminated index path".into()))?;
        let path = data[offset..offset + length].to_vec();
        validate_path(&path)?;
        if usize::from(flags & 0x0fff) != path.len().min(0x0fff) {
            return Err(Error::Invalid("index path length mismatch".into()));
        }
        if entries.last().is_some_and(|previous| previous.path >= path) {
            return Err(Error::Invalid(
                "index entries are not strictly sorted".into(),
            ));
        }
        offset += length + 1;
        let padding = (8 - (offset - start) % 8) % 8;
        if offset + padding > end || data[offset..offset + padding].iter().any(|b| *b != 0) {
            return Err(Error::Invalid("invalid index padding".into()));
        }
        offset += padding;
        if !matches!(stat[6], 0o100644 | 0o100755 | 0o120000) {
            return Err(Error::Unsupported("index entry mode".into()));
        }
        entries.push(Entry { stat, id, path });
    }
    // Dropping a TREE cache is safe; other extensions need explicit analysis.
    while offset < end {
        if end - offset < 8 {
            return Err(Error::Invalid("truncated index extension".into()));
        }
        let signature = &data[offset..offset + 4];
        let size = u32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if size > end - offset {
            return Err(Error::Invalid("truncated index extension data".into()));
        }
        if signature != b"TREE" {
            return Err(Error::Unsupported(format!(
                "index extension {:?}",
                signature
            )));
        }
        offset += size;
    }
    Ok(entries)
}

pub fn encode(hash: HashKind, entries: &[Entry]) -> Result<Vec<u8>> {
    let mut data = b"DIRC".to_vec();
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(
        &u32::try_from(entries.len())
            .map_err(|_| Error::Invalid("too many index entries".into()))?
            .to_be_bytes(),
    );
    let mut previous: Option<&[u8]> = None;
    for entry in entries {
        validate_path(&entry.path)?;
        if previous.is_some_and(|p| p >= entry.path.as_slice()) {
            return Err(Error::Invalid("unsorted index".into()));
        }
        previous = Some(&entry.path);
        if entry.id.kind() != hash {
            return Err(Error::Invalid("index object algorithm mismatch".into()));
        }
        let start = data.len();
        for field in entry.stat {
            data.extend_from_slice(&field.to_be_bytes());
        }
        data.extend_from_slice(entry.id.bytes());
        data.extend_from_slice(&(entry.path.len().min(0x0fff) as u16).to_be_bytes());
        data.extend_from_slice(&entry.path);
        data.push(0);
        while !(data.len() - start).is_multiple_of(8) {
            data.push(0);
        }
    }
    let checksum = hash.digest(&data)?;
    data.extend_from_slice(checksum.bytes());
    Ok(data)
}

pub fn validate_path(path: &[u8]) -> Result<()> {
    if path.is_empty()
        || path.contains(&0)
        || path.contains(&b'\\')
        || path.split(|b| *b == b'/').any(|part| {
            part.is_empty() || part == b"." || part == b".." || part.eq_ignore_ascii_case(b".git")
        })
    {
        return Err(Error::Invalid("unsafe repository path".into()));
    }
    Ok(())
}

/// Stages the whole worktree (`add -A` from its root). Explicit pathspecs,
/// submodules, filters and attribute conversions remain unsupported.
pub fn add_all(repo: &Repository) -> Result<usize> {
    let root = repo
        .worktree
        .as_ref()
        .ok_or_else(|| Error::Invalid("this operation must be run in a work tree".into()))?;
    let lock = LockFile::acquire(&repo.git_dir.join("index"))?;
    let previous = read(repo)?;
    let tracked: BTreeMap<_, _> = previous
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let mut paths = BTreeMap::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(|entry| !entry.file_name().eq_ignore_ascii_case(".git"))
        .build();
    for entry in walker {
        let entry = entry.map_err(|error| Error::Invalid(error.to_string()))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let kind = entry
            .file_type()
            .ok_or_else(|| Error::Invalid("unknown file type".into()))?;
        if kind.is_dir() {
            if path.join(".git").exists() {
                return Err(Error::Unsupported("nested repositories/submodules".into()));
            }
            continue;
        }
        if path.file_name().is_some_and(|s| s == ".gitattributes") {
            return Err(Error::Unsupported(
                "attribute conversions during add".into(),
            ));
        }
        if !kind.is_file() && !kind.is_symlink() {
            return Err(Error::Unsupported("special worktree files".into()));
        }
        paths.insert(
            path_bytes(path.strip_prefix(root).unwrap())?,
            path.to_path_buf(),
        );
    }
    // Tracked files remain tracked even if a new ignore rule excludes them.
    for path in tracked.keys() {
        let absolute = root.join(git_platform::path_from_bytes(path)?);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                paths.insert(path.clone(), absolute);
            }
            Ok(_) => {
                return Err(Error::Unsupported(
                    "tracked file/directory replacement".into(),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for (key, _) in [("core.autocrlf", false), ("core.sparsecheckout", false)] {
        if repo.config.boolean(key, false)? {
            return Err(Error::Unsupported(key.into()));
        }
    }
    if repo.git_dir.join("info/attributes").exists()
        || repo.config.get("core.attributesfile").is_some()
    {
        return Err(Error::Unsupported("repository/global attributes".into()));
    }
    let mut entries = Vec::new();
    for (relative, path) in paths {
        validate_path(&relative)?;
        let metadata = fs::symlink_metadata(&path)?;
        let symlink = metadata.file_type().is_symlink();
        let bytes = if symlink {
            path_bytes(&fs::read_link(&path)?)?
        } else {
            read_bounded(fs::File::open(&path)?, MAX_OBJECT_BYTES)?
        };
        let id = repo.objects.write(&Object {
            kind: ObjectKind::Blob,
            data: bytes,
        })?;
        let mut stat = stat_fields(&metadata)?;
        if symlink {
            stat[6] = 0o120000;
        }
        entries.push(Entry {
            stat,
            id,
            path: relative,
        });
    }
    let data = encode(repo.objects.hash, &entries)?;
    lock.commit(&repo.git_dir.join("index"), &data)?;
    Ok(entries.len())
}

fn stat_fields(metadata: &fs::Metadata) -> Result<[u32; 10]> {
    let mut stat = [0; 10];
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        stat[0] = metadata.ctime() as u32;
        stat[1] = metadata.ctime_nsec() as u32;
        stat[2] = metadata.mtime() as u32;
        stat[3] = metadata.mtime_nsec() as u32;
        stat[4] = metadata.dev() as u32;
        stat[5] = metadata.ino() as u32;
        stat[6] = if metadata.mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        };
        stat[7] = metadata.uid();
        stat[8] = metadata.gid();
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Unsupported("pre-epoch timestamps".into()))?;
        stat[2] = modified.as_secs() as u32;
        stat[3] = modified.subsec_nanos();
        stat[6] = 0o100644;
    }
    stat[9] =
        u32::try_from(metadata.len()).map_err(|_| Error::Unsupported("files over 4 GiB".into()))?;
    Ok(stat)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_corruption_and_paths_before_use() {
        for hash in [HashKind::Sha1, HashKind::Sha256] {
            let entry = Entry {
                stat: [0; 10],
                id: hash.digest(b"fixture").unwrap(),
                path: b"safe/file".to_vec(),
            };
            let mut entry = entry;
            entry.stat[6] = 0o100644;
            let mut bytes = encode(hash, &[entry]).unwrap();
            assert_eq!(decode(hash, &bytes).unwrap()[0].path, b"safe/file");
            bytes[20] ^= 1;
            assert!(decode(hash, &bytes).is_err());
        }
        for path in [
            b"../config".as_slice(),
            b".git/config",
            b"a/../../b",
            b"/absolute",
            b"a\\b",
        ] {
            assert!(validate_path(path).is_err());
        }
    }
}

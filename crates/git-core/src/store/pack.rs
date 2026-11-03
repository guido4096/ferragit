// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Checksummed pack readers. Index parsing and delta bytecode live separately.
mod delta;
mod index;

use super::{read_bounded, ObjectStore, ReadContext, MAX_DELTA_DEPTH, MAX_OBJECT_BYTES};
use crate::{
    object::{HashKind, Object, ObjectId, ObjectKind},
    Error, Result,
};
use flate2::bufread::ZlibDecoder;
use index::Index;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub(super) const MAX_PACK_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PACK_FILES: usize = 1024;

fn invalid(message: &str) -> Error {
    Error::Invalid(format!("corrupt pack: {message}"))
}

fn index_paths(store: &ObjectStore) -> Result<Vec<PathBuf>> {
    let directory = match fs::read_dir(store.directory.join("pack")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for entry in directory {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "idx") {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("pack-"))
        else {
            continue;
        };
        if name.len() != store.hash.hex_len() || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        paths.push(path);
        if paths.len() > MAX_PACK_FILES {
            return Err(Error::Unsupported("more than 1,024 pack indexes".into()));
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_index(path: &Path, hash: HashKind, context: &mut ReadContext) -> Result<Index> {
    let bytes = read_bounded(fs::File::open(path)?, MAX_INDEX_BYTES)?;
    context.charge_input(bytes.len())?;
    Index::parse(hash, &bytes)
}

pub(super) fn read(
    store: &ObjectStore,
    id: &ObjectId,
    context: &mut ReadContext,
    depth: usize,
) -> Result<Option<Object>> {
    for path in index_paths(store)? {
        let index = read_index(&path, store.hash, context)?;
        if let Some(slot) = index.find(id) {
            let pack = Pack::load(&path, index, store.hash, context)?;
            return Ok(Some(pack.object(slot, store, context, depth)?));
        }
    }
    Ok(None)
}

/// Two distinct matches suffice to report ambiguity; never collect an unbounded
/// list when a very short prefix matches many objects or repeated packs.
pub(super) fn matching(store: &ObjectStore, prefix: &str) -> Result<Vec<ObjectId>> {
    let mut context = ReadContext::new();
    let mut matches = BTreeMap::new();
    for path in index_paths(store)? {
        let index = read_index(&path, store.hash, &mut context)?;
        for entry in index.entries {
            let name = entry.id.to_string();
            if name.starts_with(prefix) {
                matches.insert(name, entry.id);
            }
            if matches.len() == 2 {
                return Ok(matches.into_values().collect());
            }
        }
    }
    Ok(matches.into_values().collect())
}

struct Pack {
    path: PathBuf,
    index: Index,
    bytes: Vec<u8>,
    offsets: BTreeMap<u64, usize>,
}

impl Pack {
    fn load(
        index_path: &Path,
        index: Index,
        hash: HashKind,
        context: &mut ReadContext,
    ) -> Result<Self> {
        let path = index_path.with_extension("pack");
        let bytes = read_bounded(fs::File::open(&path)?, MAX_PACK_BYTES)?;
        context.charge_input(bytes.len())?;
        let width = hash.hex_len() / 2;
        if bytes.len() < 12 + width || &bytes[..4] != b"PACK" {
            return Err(invalid("invalid header"));
        }
        let version = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        if !matches!(version, 2 | 3) {
            return Err(Error::Unsupported(format!("pack version {version}")));
        }
        let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if count != index.entries.len() {
            return Err(invalid("object count differs from index"));
        }
        let end = bytes.len() - width;
        if hash.digest(&bytes[..end])?.bytes() != &bytes[end..]
            || index.pack_checksum.bytes() != &bytes[end..]
        {
            return Err(invalid("checksum differs from data or index"));
        }
        let mut offsets = BTreeMap::new();
        for (slot, entry) in index.entries.iter().enumerate() {
            if entry.offset < 12 || entry.offset >= end as u64 {
                return Err(invalid("indexed offset outside object data"));
            }
            offsets.insert(entry.offset, slot);
        }
        if (count == 0 && end != 12) || offsets.keys().next().is_some_and(|offset| *offset != 12) {
            return Err(invalid("unindexed bytes after pack header"));
        }
        Ok(Self {
            path,
            index,
            bytes,
            offsets,
        })
    }

    fn object(
        &self,
        slot: usize,
        store: &ObjectStore,
        context: &mut ReadContext,
        depth: usize,
    ) -> Result<Object> {
        if depth > MAX_DELTA_DEPTH {
            return Err(Error::Unsupported("delta chain exceeds 64 links".into()));
        }
        let entry = &self.index.entries[slot];
        let key = (self.path.clone(), entry.offset);
        if !context.active_offsets.insert(key.clone()) {
            return Err(invalid("cyclic delta chain"));
        }
        let result = self.decode(slot, store, context, depth);
        context.active_offsets.remove(&key);
        result
    }

    fn decode(
        &self,
        slot: usize,
        store: &ObjectStore,
        context: &mut ReadContext,
        depth: usize,
    ) -> Result<Object> {
        let entry = &self.index.entries[slot];
        let start = entry.offset as usize; // load established offset < bounded Vec length.
        let end = self
            .offsets
            .range((
                std::ops::Bound::Excluded(entry.offset),
                std::ops::Bound::Unbounded,
            ))
            .next()
            .map(|(offset, _)| *offset as usize)
            .unwrap_or(self.bytes.len() - store.hash.hex_len() / 2);
        let raw = &self.bytes[start..end];
        if entry.crc.is_some_and(|crc| crc32fast::hash(raw) != crc) {
            return Err(invalid("object CRC mismatch"));
        }
        let mut input = raw;
        let (&first, rest) = input.split_first().ok_or_else(|| invalid("empty entry"))?;
        input = rest;
        let kind = (first >> 4) & 7;
        let mut size = u64::from(first & 15);
        let mut shift = 4;
        let mut byte = first;
        while byte & 128 != 0 {
            let (&next, rest) = input
                .split_first()
                .ok_or_else(|| invalid("truncated object size"))?;
            input = rest;
            byte = next;
            let chunk = u64::from(byte & 127);
            if shift >= 64 || chunk > (u64::MAX >> shift) {
                return Err(invalid("object size overflow"));
            }
            size |= chunk << shift;
            shift += 7;
        }
        if size > MAX_OBJECT_BYTES {
            return Err(Error::Unsupported(
                "packed object/delta exceeds 64 MiB".into(),
            ));
        }
        let base = match kind {
            1..=4 => Base::None,
            6 => {
                let distance = offset_distance(&mut input)?;
                let offset = entry
                    .offset
                    .checked_sub(distance)
                    .filter(|offset| *offset >= 12 && *offset < entry.offset)
                    .ok_or_else(|| invalid("delta base offset outside preceding objects"))?;
                let slot = *self
                    .offsets
                    .get(&offset)
                    .ok_or_else(|| invalid("delta base is not an indexed object"))?;
                Base::Slot(slot)
            }
            7 => {
                let width = store.hash.hex_len() / 2;
                if input.len() < width {
                    return Err(invalid("truncated delta base ID"));
                }
                let id = ObjectId::from_bytes(store.hash, &input[..width])?;
                input = &input[width..];
                match self.index.find(&id) {
                    Some(slot) => Base::Slot(slot),
                    None => Base::Id(id),
                }
            }
            _ => return Err(invalid("invalid object type")),
        };
        context.charge_output(size as usize)?;
        let mut decoder = ZlibDecoder::new(input);
        let data = read_bounded(&mut decoder, size)?;
        if data.len() as u64 != size || decoder.total_in() != input.len() as u64 {
            return Err(invalid("zlib length or entry boundary mismatch"));
        }
        let object = match base {
            Base::None => Object {
                kind: match kind {
                    1 => ObjectKind::Commit,
                    2 => ObjectKind::Tree,
                    3 => ObjectKind::Blob,
                    4 => ObjectKind::Tag,
                    _ => unreachable!(),
                },
                data,
            },
            other => {
                let result_size = delta::result_size(&data, MAX_OBJECT_BYTES)?;
                context.charge_output(result_size)?;
                let base = match other {
                    Base::Slot(slot) => self.object(slot, store, context, depth + 1)?,
                    Base::Id(id) => store.read_with_context(&id, context, depth + 1)?,
                    Base::None => unreachable!(),
                };
                Object {
                    kind: base.kind,
                    data: delta::apply(&base.data, &data, MAX_OBJECT_BYTES)?,
                }
            }
        };
        if store.hash.digest(&object.encode())? != entry.id {
            return Err(invalid("reconstructed object ID mismatch"));
        }
        Ok(object)
    }
}

enum Base {
    None,
    Slot(usize),
    Id(ObjectId),
}

fn offset_distance(input: &mut &[u8]) -> Result<u64> {
    let (&first, rest) = input
        .split_first()
        .ok_or_else(|| invalid("truncated delta offset"))?;
    *input = rest;
    let mut byte = first;
    let mut distance = u64::from(byte & 127);
    while byte & 128 != 0 {
        let (&next, rest) = input
            .split_first()
            .ok_or_else(|| invalid("truncated delta offset"))?;
        *input = rest;
        byte = next;
        distance = distance
            .checked_add(1)
            .and_then(|d| d.checked_mul(128))
            .and_then(|d| d.checked_add(u64::from(byte & 127)))
            .ok_or_else(|| invalid("delta offset overflow"))?;
    }
    if distance == 0 {
        return Err(invalid("zero delta offset"));
    }
    Ok(distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offset_encoding_is_distinct_from_size_encoding() {
        assert_eq!(offset_distance(&mut &[0x80, 0][..]).unwrap(), 128);
        assert_eq!(offset_distance(&mut &[0x81, 0][..]).unwrap(), 256);
        assert!(offset_distance(&mut &[0][..]).is_err());
        assert!(offset_distance(&mut &[0xff; 20][..]).is_err());
    }
}

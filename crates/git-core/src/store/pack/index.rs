// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Version 1/2 pack indexes, including SHA-256 and escaped 64-bit offsets.
use crate::{
    object::{HashKind, ObjectId},
    Error, Result,
};
use std::collections::BTreeSet;

pub(super) const MAX_OBJECTS: usize = 100_000;

#[derive(Debug)]
pub(super) struct Entry {
    pub id: ObjectId,
    pub offset: u64,
    pub crc: Option<u32>,
}

pub(super) struct Index {
    pub entries: Vec<Entry>,
    pub pack_checksum: ObjectId,
}

fn invalid(message: &str) -> Error {
    Error::Invalid(format!("corrupt pack index: {message}"))
}

fn number(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

impl Index {
    pub fn parse(hash: HashKind, bytes: &[u8]) -> Result<Self> {
        let width = hash.hex_len() / 2;
        if bytes.len() < 1024 + 2 * width {
            return Err(invalid("truncated header"));
        }
        let end = bytes.len() - width;
        if hash.digest(&bytes[..end])?.bytes() != &bytes[end..] {
            return Err(invalid("checksum mismatch"));
        }
        let (version, start) = if bytes.starts_with(b"\xfftOc") {
            let version = number(bytes, 4);
            if version != 2 {
                return Err(Error::Unsupported(format!("pack index version {version}")));
            }
            (2, 8)
        } else {
            (1, 0)
        };
        if bytes.len() < start + 1024 + 2 * width {
            return Err(invalid("truncated fanout table"));
        }
        let mut fanout = [0u32; 256];
        let mut previous = 0;
        for (i, value) in fanout.iter_mut().enumerate() {
            *value = number(bytes, start + i * 4);
            if *value < previous {
                return Err(invalid("non-monotonic fanout"));
            }
            previous = *value;
        }
        let count = fanout[255] as usize;
        if count > MAX_OBJECTS {
            return Err(Error::Unsupported(
                "pack index exceeds 100,000 objects".into(),
            ));
        }
        let table = start + 1024;
        let stride = if version == 1 { width + 4 } else { width + 8 };
        let minimum = table + count * stride + 2 * width;
        if bytes.len() < minimum {
            return Err(invalid("truncated object tables"));
        }
        let large = table + count * stride;
        let large_count = (bytes.len() - minimum) / 8;
        if (version == 1 && bytes.len() != minimum)
            || (version == 2 && !(bytes.len() - minimum).is_multiple_of(8))
        {
            return Err(invalid("unexpected trailing data"));
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(count);
        let mut actual_fanout = [0u32; 256];
        let mut offsets = BTreeSet::new();
        let mut large_used = BTreeSet::new();
        for i in 0..count {
            let (id_at, raw, crc) = if version == 1 {
                (
                    table + i * stride + 4,
                    number(bytes, table + i * stride),
                    None,
                )
            } else {
                (
                    table + i * width,
                    number(bytes, table + count * (width + 4) + i * 4),
                    Some(number(bytes, table + count * width + i * 4)),
                )
            };
            let id = ObjectId::from_bytes(hash, &bytes[id_at..id_at + width])?;
            if entries
                .last()
                .is_some_and(|entry| entry.id.bytes() >= id.bytes())
            {
                return Err(invalid("unsorted or duplicate object IDs"));
            }
            actual_fanout[usize::from(id.bytes()[0])] += 1;
            let offset = if version == 2 && raw & 0x8000_0000 != 0 {
                let slot = (raw & 0x7fff_ffff) as usize;
                if slot >= large_count || !large_used.insert(slot) {
                    return Err(invalid("invalid 64-bit offset slot"));
                }
                let at = large + slot * 8;
                u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap())
            } else {
                u64::from(raw)
            };
            if offset < 12 || !offsets.insert(offset) {
                return Err(invalid("invalid or duplicate pack offset"));
            }
            entries.push(Entry { id, offset, crc });
        }
        if large_used.len() != large_count {
            return Err(invalid("unused 64-bit offsets"));
        }
        let mut total = 0;
        for i in 0..256 {
            total += actual_fanout[i];
            if total != fanout[i] {
                return Err(invalid("fanout does not match object IDs"));
            }
        }
        let pack_checksum = ObjectId::from_bytes(hash, &bytes[end - width..end])?;
        Ok(Self {
            entries,
            pack_checksum,
        })
    }

    pub fn find(&self, id: &ObjectId) -> Option<usize> {
        self.entries
            .binary_search_by(|entry| entry.id.bytes().cmp(id.bytes()))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checksum(hash: HashKind, mut bytes: Vec<u8>) -> Vec<u8> {
        let id = hash.digest(&bytes).unwrap();
        bytes.extend_from_slice(id.bytes());
        bytes
    }
    fn fixture(hash: HashKind, version: u32, large: bool) -> Vec<u8> {
        let id = hash.digest(b"id fixture").unwrap();
        let mut bytes = Vec::new();
        if version == 2 {
            bytes.extend(b"\xfftOc");
            bytes.extend(2u32.to_be_bytes());
        }
        for i in 0..256 {
            bytes.extend(u32::from(i >= usize::from(id.bytes()[0])).to_be_bytes());
        }
        if version == 1 {
            bytes.extend(12u32.to_be_bytes());
            bytes.extend(id.bytes());
        } else {
            bytes.extend(id.bytes());
            bytes.extend(0u32.to_be_bytes());
            bytes.extend(if large { 0x8000_0000u32 } else { 12u32 }.to_be_bytes());
            if large {
                bytes.extend(12u64.to_be_bytes());
            }
        }
        bytes.extend(vec![0; hash.hex_len() / 2]);
        checksum(hash, bytes)
    }
    #[test]
    fn both_hashes_and_versions_and_wide_offsets() {
        for hash in [HashKind::Sha1, HashKind::Sha256] {
            for (version, large) in [(1, false), (2, false), (2, true)] {
                let parsed = Index::parse(hash, &fixture(hash, version, large)).unwrap();
                assert_eq!(parsed.entries[0].offset, 12);
                assert_eq!(parsed.find(&hash.digest(b"id fixture").unwrap()), Some(0));
            }
        }
    }
    #[test]
    fn rejects_truncation_checksums_fanout_and_offset_slots() {
        let hash = HashKind::Sha1;
        let bytes = fixture(hash, 2, true);
        for length in 0..bytes.len() {
            assert!(Index::parse(hash, &bytes[..length]).is_err());
        }
        let mut bad = bytes.clone();
        bad[20] ^= 1;
        assert!(Index::parse(hash, &bad).is_err());
        let mut bad = bytes[..bytes.len() - 20].to_vec();
        bad[8 + 255 * 4..8 + 256 * 4].copy_from_slice(&2u32.to_be_bytes());
        assert!(Index::parse(hash, &checksum(hash, bad)).is_err());
        let mut bad = bytes[..bytes.len() - 20].to_vec();
        let slot = 8 + 1024 + 20 + 4;
        bad[slot..slot + 4].copy_from_slice(&0x8000_0001u32.to_be_bytes());
        assert!(Index::parse(hash, &checksum(hash, bad)).is_err());
    }
}

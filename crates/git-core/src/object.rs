// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Pure object framing, IDs, and validation. Object paths are never supplied by callers.
use crate::{Error, Result};
use sha1_checked::Sha1;
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HashKind {
    Sha1,
    Sha256,
}

impl HashKind {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "sha1" => Ok(Self::Sha1),
            "sha256" => Ok(Self::Sha256),
            _ => Err(Error::Invalid(format!("unknown hash algorithm '{name}'"))),
        }
    }
    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    pub fn digest(self, bytes: &[u8]) -> Result<ObjectId> {
        let digest = match self {
            Self::Sha1 => {
                let output = Sha1::try_digest(bytes);
                if output.has_collision() {
                    return Err(Error::Invalid("SHA-1 collision detected".into()));
                }
                output.hash().to_vec()
            }
            Self::Sha256 => Sha256::digest(bytes).to_vec(),
        };
        Ok(ObjectId {
            kind: self,
            bytes: digest,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectId {
    kind: HashKind,
    bytes: Vec<u8>,
}

impl ObjectId {
    pub fn from_bytes(kind: HashKind, bytes: &[u8]) -> Result<Self> {
        if bytes.len() != kind.hex_len() / 2 {
            return Err(Error::Invalid("object ID byte width mismatch".into()));
        }
        Ok(Self {
            kind,
            bytes: bytes.to_vec(),
        })
    }

    pub fn parse(kind: HashKind, text: &str) -> Result<Self> {
        if text.len() != kind.hex_len() || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!(
                "invalid {} object ID '{text}'",
                kind.name()
            )));
        }
        let bytes = text
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                // The ASCII and length checks above establish both nibble preconditions.
                let high = (pair[0] as char).to_digit(16).unwrap() as u8;
                let low = (pair[1] as char).to_digit(16).unwrap() as u8;
                high * 16 + low
            })
            .collect();
        Ok(Self { kind, bytes })
    }
    pub fn kind(&self) -> HashKind {
        self.kind
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.bytes {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl ObjectKind {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "blob" => Ok(Self::Blob),
            "tree" => Ok(Self::Tree),
            "commit" => Ok(Self::Commit),
            "tag" => Ok(Self::Tag),
            _ => Err(Error::Invalid(format!("invalid object type \"{text}\""))),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Tree => "tree",
            Self::Commit => "commit",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug)]
pub struct Object {
    pub kind: ObjectKind,
    pub data: Vec<u8>,
}

impl Object {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = format!("{} {}\0", self.kind.name(), self.data.len()).into_bytes();
        bytes.extend_from_slice(&self.data);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| Error::Invalid("corrupt object: missing header terminator".into()))?;
        let header = std::str::from_utf8(&bytes[..end])
            .map_err(|_| Error::Invalid("corrupt object header".into()))?;
        let (kind, size) = header
            .split_once(' ')
            .ok_or_else(|| Error::Invalid("corrupt object header".into()))?;
        if size.is_empty()
            || !size.bytes().all(|byte| byte.is_ascii_digit())
            || (size.len() > 1 && size.starts_with('0'))
        {
            return Err(Error::Invalid("corrupt object size".into()));
        }
        let size = size
            .parse::<usize>()
            .map_err(|_| Error::Invalid("corrupt object size".into()))?;
        if size != bytes.len() - end - 1 {
            return Err(Error::Invalid("corrupt object: size mismatch".into()));
        }
        Ok(Self {
            kind: ObjectKind::parse(kind)?,
            data: bytes[end + 1..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_empty_blob_ids() {
        let bytes = Object {
            kind: ObjectKind::Blob,
            data: vec![],
        }
        .encode();
        assert_eq!(
            HashKind::Sha1.digest(&bytes).unwrap().to_string(),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            HashKind::Sha256.digest(&bytes).unwrap().to_string(),
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
        );
    }
    #[test]
    fn rejects_malformed_headers_and_sizes() {
        for bytes in [
            b"blob 2\0a".as_slice(),
            b"blob 01\0a",
            b"blob -1\0",
            b"blob 0",
            b"evil 0\0",
        ] {
            assert!(Object::decode(bytes).is_err(), "accepted {bytes:?}");
        }
    }
    #[test]
    fn ids_reject_paths_and_wrong_hash_width() {
        assert!(ObjectId::parse(HashKind::Sha1, &"../a".repeat(10)).is_err());
        assert!(ObjectId::parse(HashKind::Sha256, &"a".repeat(40)).is_err());
    }
}

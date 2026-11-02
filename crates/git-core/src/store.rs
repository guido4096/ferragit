// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Loose objects only. Stored bytes are bounded, framed, and verified against their ID.
use crate::{
    object::{HashKind, Object, ObjectId},
    Error, Result,
};
use flate2::{bufread::ZlibDecoder, write::ZlibEncoder, Compression};
use git_platform::TempFile;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024;

pub fn read_bounded(mut input: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut input).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Unsupported(format!("input exceeds {limit} bytes")));
    }
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    pub directory: PathBuf,
    pub hash: HashKind,
}

impl ObjectStore {
    pub fn new(git_dir: &Path, hash: HashKind) -> Self {
        Self {
            directory: git_dir.join("objects"),
            hash,
        }
    }

    fn path(&self, id: &ObjectId) -> Result<PathBuf> {
        if id.kind() != self.hash {
            return Err(Error::Invalid("object hash algorithm mismatch".into()));
        }
        let name = id.to_string();
        Ok(self.directory.join(&name[..2]).join(&name[2..]))
    }

    pub fn read(&self, id: &ObjectId) -> Result<Object> {
        let path = self.path(id)?;
        let file = fs::File::open(path)?;
        let compressed = read_bounded(file, MAX_OBJECT_BYTES + 1024 * 1024)?;
        let mut decoder = ZlibDecoder::new(compressed.as_slice());
        let bytes = read_bounded(&mut decoder, MAX_OBJECT_BYTES + 128)?;
        if decoder.total_in() != compressed.len() as u64 {
            return Err(Error::Invalid("trailing bytes after zlib object".into()));
        }
        let object = Object::decode(&bytes)?;
        if self.hash.digest(&bytes)? != *id {
            return Err(Error::Invalid("corrupt object: hash mismatch".into()));
        }
        Ok(object)
    }

    pub fn write(&self, object: &Object) -> Result<ObjectId> {
        if object.data.len() as u64 > MAX_OBJECT_BYTES {
            return Err(Error::Unsupported("object exceeds 64 MiB".into()));
        }
        let bytes = object.encode();
        let id = self.hash.digest(&bytes)?;
        let destination = self.path(&id)?;
        let directory = destination
            .parent()
            .expect("fanout object path has a parent");
        fs::create_dir_all(directory)?;
        let mut temporary = TempFile::in_directory(directory)?;
        let mut encoder = ZlibEncoder::new(temporary.writer(), Compression::default());
        encoder.write_all(&bytes)?;
        encoder.finish()?;
        temporary.persist_no_clobber(&destination)?;
        // Verify even a pre-existing destination: a concurrent writer may have
        // won the name, but an already corrupt object must not be reported as success.
        let stored = self.read(&id)?;
        if stored.kind != object.kind || stored.data != object.data {
            return Err(Error::Invalid("existing object differs".into()));
        }
        Ok(id)
    }

    pub fn resolve_hex(&self, name: &str) -> Result<ObjectId> {
        if name.len() == self.hash.hex_len() {
            return ObjectId::parse(self.hash, name);
        }
        if name.len() < 4
            || name.len() > self.hash.hex_len()
            || !name.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Invalid(format!("Not a valid object name {name}")));
        }
        let prefix = name.to_ascii_lowercase();
        let mut matches = Vec::new();
        if let Ok(entries) = fs::read_dir(self.directory.join(&prefix[..2])) {
            for entry in entries {
                let entry = entry?;
                if let Some(suffix) = entry.file_name().to_str() {
                    let full = format!("{}{suffix}", &prefix[..2]);
                    if full.len() == self.hash.hex_len() && full.starts_with(&prefix) {
                        if let Ok(id) = ObjectId::parse(self.hash, &full) {
                            matches.push(id);
                        }
                    }
                }
            }
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(Error::Invalid(format!("Not a valid object name {name}"))),
            _ => Err(Error::Invalid(format!(
                "short object ID {name} is ambiguous"
            ))),
        }
    }
}

// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Loose and packed object lookup. All returned objects are verified by their IDs.
mod pack;
use crate::{
    object::{HashKind, Object, ObjectId},
    Error, Result,
};
use flate2::{bufread::ZlibDecoder, write::ZlibEncoder, Compression};
use git_platform::TempFile;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DELTA_DEPTH: usize = 64;
const MAX_RESOLUTION_BYTES: usize = 256 * 1024 * 1024;

struct ReadContext {
    active_ids: BTreeSet<String>,
    active_offsets: BTreeSet<(PathBuf, u64)>,
    input_left: usize,
    output_left: usize,
}

impl ReadContext {
    fn new() -> Self {
        Self {
            active_ids: BTreeSet::new(),
            active_offsets: BTreeSet::new(),
            input_left: MAX_RESOLUTION_BYTES,
            output_left: MAX_RESOLUTION_BYTES,
        }
    }
    fn charge_input(&mut self, amount: usize) -> Result<()> {
        self.input_left = self.input_left.checked_sub(amount).ok_or_else(|| {
            Error::Unsupported("object lookup exceeds 256 MiB of pack/index reads".into())
        })?;
        Ok(())
    }
    fn charge_output(&mut self, amount: usize) -> Result<()> {
        self.output_left = self.output_left.checked_sub(amount).ok_or_else(|| {
            Error::Unsupported("object/delta resolution exceeds 256 MiB of work".into())
        })?;
        Ok(())
    }
}

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
        self.read_with_context(id, &mut ReadContext::new(), 0)
    }

    fn read_with_context(
        &self,
        id: &ObjectId,
        context: &mut ReadContext,
        depth: usize,
    ) -> Result<Object> {
        if depth > MAX_DELTA_DEPTH {
            return Err(Error::Unsupported("delta chain exceeds 64 links".into()));
        }
        let name = id.to_string();
        if !context.active_ids.insert(name.clone()) {
            return Err(Error::Invalid("cyclic delta object IDs".into()));
        }
        let result = self.read_inner(id, context, depth);
        context.active_ids.remove(&name);
        result
    }

    fn read_inner(&self, id: &ObjectId, context: &mut ReadContext, depth: usize) -> Result<Object> {
        let path = self.path(id)?;
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return pack::read(self, id, context, depth)?.ok_or_else(|| error.into());
            }
            Err(error) => return Err(error.into()),
        };
        let compressed = read_bounded(file, MAX_OBJECT_BYTES + 1024 * 1024)?;
        let mut decoder = ZlibDecoder::new(compressed.as_slice());
        let bytes = read_bounded(&mut decoder, MAX_OBJECT_BYTES + 128)?;
        if decoder.total_in() != compressed.len() as u64 {
            return Err(Error::Invalid("trailing bytes after zlib object".into()));
        }
        let object = Object::decode(&bytes)?;
        if object.data.len() as u64 > MAX_OBJECT_BYTES {
            return Err(Error::Unsupported("loose object exceeds 64 MiB".into()));
        }
        context.charge_output(bytes.len())?;
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
        let mut matches = BTreeMap::new();
        let entries = match fs::read_dir(self.directory.join(&prefix[..2])) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(entries) = entries {
            for entry in entries {
                let entry = entry?;
                if let Some(suffix) = entry.file_name().to_str() {
                    let full = format!("{}{suffix}", &prefix[..2]);
                    if full.len() == self.hash.hex_len() && full.starts_with(&prefix) {
                        if let Ok(id) = ObjectId::parse(self.hash, &full) {
                            matches.insert(full, id);
                            if matches.len() == 2 {
                                break;
                            }
                        }
                    }
                }
            }
        }
        if matches.len() < 2 {
            for id in pack::matching(self, &prefix)? {
                matches.insert(id.to_string(), id);
            }
        }
        match matches.len() {
            1 => Ok(matches.into_values().next().unwrap()),
            0 => Err(Error::Invalid(format!("Not a valid object name {name}"))),
            _ => Err(Error::Invalid(format!(
                "short object ID {name} is ambiguous"
            ))),
        }
    }
}

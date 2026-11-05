// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Single-ref deletion: symbolic logs, packed publication, then loose removal.
use super::{packed, reflog, resolve, safe_path, Update};
use crate::{identity::Identity, object::ObjectId, Error, Result};
use git_platform::LockFile;
use std::{fs, io, path::Path};

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Preserve refs/{heads,tags,...} and logs/refs/{heads,tags,...} roots.
fn remove_empty_parents(root: &Path, name: &str) {
    let parts: Vec<_> = name.split('/').collect();
    for end in (3..parts.len()).rev() {
        if fs::remove_dir(root.join(parts[..end].join("/"))).is_err() {
            break;
        }
    }
}

impl Update<'_> {
    pub fn delete(self, identity: impl FnOnce() -> Result<Identity>, reason: &[u8]) -> Result<()> {
        let repo = self.repo;
        let target = self
            .names
            .last()
            .expect("resolved chain is nonempty")
            .clone();
        safe_path(&repo.git_dir, "packed-refs")?;
        // All writers acquire ref-chain locks before the shared packed lock.
        // Keep this lock after publishing packed bytes until loose deletion ends.
        let packed_lock = LockFile::acquire(&repo.git_dir.join("packed-refs"))?;
        let current = resolve(repo, &self.names[0], 0)?;
        if current.names != self.names || current.value != self.old {
            return Err(Error::Invalid(
                "reference changed while locking packed refs".into(),
            ));
        }
        let replacement = packed::without(
            &repo.git_dir.join("packed-refs"),
            repo.objects.hash,
            &target,
        )?;
        let publication = if replacement.is_some() {
            Some(LockFile::acquire(&repo.git_dir.join("packed-refs.new"))?)
        } else {
            None
        };
        let relative_log = format!("logs/{target}");
        safe_path(&repo.git_dir, &relative_log)?;
        if repo.git_dir.join(&relative_log).is_dir() {
            return Err(Error::Unsupported(
                "directory at deleted reflog path".into(),
            ));
        }
        let mut logs = Vec::new();
        for name in &self.names {
            if name != &target && reflog::enabled(repo, name)? {
                logs.push(name.as_str());
            }
        }
        if !self.names.iter().any(|name| name == "HEAD")
            && resolve(repo, "HEAD", 0)?.names.last() == Some(&target)
            && reflog::enabled(repo, "HEAD")?
        {
            logs.push("HEAD");
        }
        if !logs.is_empty() {
            let zero = ObjectId::from_bytes(
                repo.objects.hash,
                &[0; 32][..repo.objects.hash.hex_len() / 2],
            )?;
            let record = reflog::record(repo, self.old.as_ref(), &zero, &identity()?, reason)?;
            for name in logs {
                reflog::append(repo, name, &record)?;
            }
        }
        // Git's files backend leaves earlier log effects visible if a later
        // publication/unlink fails; this is not a cross-file atomic transaction.
        remove_file(&repo.git_dir.join(&relative_log))?;
        remove_empty_parents(&repo.git_dir.join("logs"), &target);
        if let (Some(lock), Some(bytes)) = (publication, replacement) {
            lock.commit(&repo.git_dir.join("packed-refs"), &bytes)?;
        }
        remove_file(&repo.git_dir.join(&target))?;
        drop(self); // Release owned ref locks before removing empty directories.
        remove_empty_parents(&repo.git_dir, &target);
        drop(packed_lock);
        Ok(())
    }
}

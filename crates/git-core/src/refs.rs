// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Bounded files-backend reads and single-reference updates. Log appends and ref
//! publication have distinct failure boundaries, matching the reference backend.
mod packed;
mod reflog;

use crate::{
    identity::Identity,
    object::{ObjectId, ObjectKind},
    repository::Repository,
    store::read_bounded,
    Error, Result,
};
use git_platform::LockFile;
use std::{collections::BTreeSet, fs, io, path::Path};

const MAX_REF_BYTES: u64 = 4096;
const MAX_SYMREF_LINKS: usize = 8;

pub fn valid_ref(name: &str) -> bool {
    name.starts_with("refs/")
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains(['\\', ' ', '~', '^', ':', '?', '*', '[', '\x7f'])
        && !name.chars().any(char::is_control)
        && !name.contains("..")
        && !name.contains("@{")
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn validate(name: &str) -> Result<()> {
    if (name == "HEAD" || valid_ref(name)) && name.len() <= MAX_REF_BYTES as usize {
        Ok(())
    } else {
        Err(Error::Invalid(format!("invalid reference name '{name}'")))
    }
}

#[derive(Debug)]
struct Resolved {
    names: Vec<String>,
    value: Option<ObjectId>,
}

fn resolve(repo: &Repository, name: &str, depth: usize) -> Result<Resolved> {
    validate(name)?;
    let mut names = Vec::new();
    let mut current = name.to_owned();
    loop {
        if names.len() + depth > MAX_SYMREF_LINKS || names.contains(&current) {
            return Err(Error::Invalid("invalid or recursive reference".into()));
        }
        names.push(current.clone());
        let bytes = match fs::File::open(repo.git_dir.join(&current)) {
            Ok(file) => Some(read_bounded(file, MAX_REF_BYTES)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let value = if let Some(bytes) = bytes {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| Error::Invalid("non-UTF-8 loose reference".into()))?;
            if let Some(target) = text.trim().strip_prefix("ref: ") {
                if !valid_ref(target) {
                    return Err(Error::Invalid("invalid symbolic reference target".into()));
                }
                validate(target)?;
                current = target.to_owned();
                continue;
            }
            Some(ObjectId::parse(repo.objects.hash, text.trim())?)
        } else {
            packed::lookup(
                &repo.git_dir.join("packed-refs"),
                repo.objects.hash,
                &current,
            )?
        };
        if value
            .as_ref()
            .is_some_and(|id| id.bytes().iter().all(|byte| *byte == 0))
        {
            return Err(Error::Invalid("null object ID in reference".into()));
        }
        return Ok(Resolved { names, value });
    }
}

pub fn read(repo: &Repository, name: &str, depth: u8) -> Result<Option<ObjectId>> {
    Ok(resolve(repo, name, usize::from(depth))?.value)
}

pub fn head_target(repo: &Repository) -> Result<String> {
    let resolved = resolve(repo, "HEAD", 0)?;
    if resolved.names.len() == 1 {
        return Err(Error::Unsupported("detached HEAD commits".into()));
    }
    resolved
        .names
        .last()
        .cloned()
        .ok_or_else(|| Error::Invalid("missing HEAD target".into()))
}

/// Reject unsupported filesystem indirections before creating locks or logs.
/// This is not an ownership check or protection against hostile directory races.
fn safe_path(root: &Path, relative: &str) -> Result<()> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::Unsupported(
                    "symlinks in reference or reflog write paths".into(),
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn hooks_directory(repo: &Repository) -> std::path::PathBuf {
    match repo.config.get("core.hookspath") {
        Some(path) => repo.worktree.as_ref().unwrap_or(&repo.git_dir).join(path),
        None => repo.git_dir.join("hooks"),
    }
}

pub enum Expected {
    Any,
    Missing,
    Value(ObjectId),
}

/// Owns all locks in the selected symbolic-ref chain until completion or drop.
pub struct Update<'a> {
    repo: &'a Repository,
    names: Vec<String>,
    old: Option<ObjectId>,
    locks: Vec<(String, LockFile)>,
}

impl<'a> Update<'a> {
    pub fn lock(repo: &'a Repository, reference: &str, expected: Expected) -> Result<Self> {
        if hooks_directory(repo).join("reference-transaction").exists() {
            return Err(Error::Unsupported("reference-transaction hook".into()));
        }
        let before = resolve(repo, reference, 0)?;
        let mut locks = Vec::new();
        // Stable ordering avoids lock-order inversions between symbolic chains.
        for name in before.names.iter().collect::<BTreeSet<_>>() {
            safe_path(&repo.git_dir, name)?;
            let path = repo.git_dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            locks.push((name.clone(), LockFile::acquire(&path)?));
        }
        let after = resolve(repo, reference, 0)?;
        if before.names != after.names {
            return Err(Error::Invalid(
                "symbolic reference changed while acquiring locks".into(),
            ));
        }
        let mismatch = match &expected {
            Expected::Any => None,
            Expected::Missing if after.value.is_none() => None,
            Expected::Missing => Some("reference already exists".to_owned()),
            Expected::Value(id) if id.kind() != repo.objects.hash => {
                return Err(Error::Invalid(
                    "expected reference hash algorithm mismatch".into(),
                ))
            }
            Expected::Value(id) if after.value.as_ref() == Some(id) => None,
            Expected::Value(id) => Some(match &after.value {
                Some(actual) => format!("is at {actual} but expected {id}"),
                None => format!("unable to resolve reference '{reference}'"),
            }),
        };
        if let Some(reason) = mismatch {
            return Err(Error::Invalid(format!(
                "cannot lock ref '{reference}': {reason}"
            )));
        }
        Ok(Self {
            repo,
            names: after.names,
            old: after.value,
            locks,
        })
    }

    pub fn old(&self) -> Option<&ObjectId> {
        self.old.as_ref()
    }

    pub fn finish(
        mut self,
        new: &ObjectId,
        identity: impl FnOnce() -> Result<Identity>,
        reason: &[u8],
    ) -> Result<()> {
        if new.kind() != self.repo.objects.hash {
            return Err(Error::Invalid(
                "new reference hash algorithm mismatch".into(),
            ));
        }
        if new.bytes().iter().all(|byte| *byte == 0) {
            return Err(Error::Unsupported("reference deletion".into()));
        }
        // A resolved chain always contains its starting reference.
        let target = self
            .names
            .last()
            .expect("resolved reference chain is nonempty");
        let object = self.repo.objects.read(new).map_err(|error| match error {
            Error::Io(ref io) if io.kind() == io::ErrorKind::NotFound => Error::Invalid(format!(
                "cannot update ref '{0}': trying to write ref '{0}' with nonexistent object {new}",
                self.names[0]
            )),
            other => other,
        })?;
        if target.starts_with("refs/heads/") && object.kind != ObjectKind::Commit {
            return Err(Error::Invalid(format!(
                "cannot update ref '{}': trying to write non-commit object {new} to branch '{}'",
                self.names[0], self.names[0]
            )));
        }
        let changed = self.old.as_ref() != Some(new);
        let mut logs = Vec::new();
        for name in self.names.iter().collect::<BTreeSet<_>>() {
            if (name != target || changed) && reflog::enabled(self.repo, name)? {
                logs.push(name.clone());
            }
        }
        let extra_head = if self.names.iter().any(|name| name == "HEAD") {
            false
        } else {
            let head = resolve(self.repo, "HEAD", 0)?;
            head.names.last() == Some(target) && reflog::enabled(self.repo, "HEAD")?
        };
        let line = if !logs.is_empty() || extra_head {
            Some(reflog::record(
                self.repo,
                self.old.as_ref(),
                new,
                &identity()?,
                reason,
            )?)
        } else {
            None
        };
        // Symbolic logs precede the terminal ref's publication. Earlier log
        // appends are intentionally not rolled back if a later append fails.
        for name in logs {
            reflog::append(
                self.repo,
                &name,
                line.as_deref().expect("enabled log has a record"),
            )?;
        }
        if changed {
            let index = self
                .locks
                .iter()
                .position(|(name, _)| name == target)
                .expect("terminal ref lock is owned");
            let (_, lock) = self.locks.remove(index);
            lock.commit(
                &self.repo.git_dir.join(target),
                format!("{new}\n").as_bytes(),
            )?;
        }
        // The files backend updates an associated HEAD log after publishing a
        // directly named branch. Failure here reports an error with the ref
        // already advanced; callers must not retry assuming no change occurred.
        if extra_head {
            reflog::append(
                self.repo,
                "HEAD",
                line.as_deref().expect("enabled HEAD log has a record"),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_names_cannot_escape_the_repository() {
        for name in [
            "refs/heads/../config",
            "refs/heads/x.lock",
            "refs/heads/x\\y",
            "refs//x",
            "refs/heads/.x",
            "refs/heads/x\ny",
        ] {
            assert!(!valid_ref(name));
        }
        assert!(valid_ref("refs/heads/feature/topic"));
    }
}

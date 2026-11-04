// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Files-backend reflog policy, byte framing, and durable append operations.
use super::safe_path;
use crate::{
    identity::Identity, object::ObjectId, repository::Repository, store::MAX_OBJECT_BYTES, Error,
    Result,
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
};

pub(super) fn enabled(repo: &Repository, name: &str) -> Result<bool> {
    let automatic = match repo
        .config
        .get("core.logallrefupdates")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("always") => true,
        _ => {
            repo.config.boolean(
                "core.logallrefupdates",
                !repo.config.boolean("core.bare", false)?,
            )? && (name == "HEAD"
                || ["refs/heads/", "refs/remotes/", "refs/notes/"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix)))
        }
    };
    let relative = format!("logs/{name}");
    safe_path(&repo.git_dir, &relative)?;
    Ok(automatic || repo.git_dir.join(relative).try_exists()?)
}

fn normalize(reason: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut space = false;
    for byte in reason.iter().copied() {
        if byte.is_ascii_whitespace() {
            space = !result.is_empty();
        } else {
            if space {
                result.push(b' ');
                space = false;
            }
            result.push(byte);
        }
    }
    result
}

pub(super) fn record(
    repo: &Repository,
    old: Option<&ObjectId>,
    new: &ObjectId,
    identity: &Identity,
    reason: &[u8],
) -> Result<Vec<u8>> {
    identity.validate()?;
    if reason.len() as u64 > MAX_OBJECT_BYTES || reason.contains(&0) {
        return Err(Error::Invalid("invalid or excessive reflog message".into()));
    }
    let old = old
        .map(ToString::to_string)
        .unwrap_or_else(|| "0".repeat(repo.objects.hash.hex_len()));
    let mut line = format!("{old} {new} {}", identity.line()).into_bytes();
    let reason = normalize(reason);
    if !reason.is_empty() {
        line.push(b'\t');
        line.extend(reason);
    }
    line.push(b'\n');
    Ok(line)
}

pub(super) fn append(repo: &Repository, name: &str, record: &[u8]) -> Result<()> {
    let relative = format!("logs/{name}");
    safe_path(&repo.git_dir, &relative)?;
    let path = repo.git_dir.join(&relative);
    if path.is_dir() {
        // Git replaces an empty log directory, but a directory containing logs
        // is a namespace conflict and must not be removed recursively.
        fs::remove_dir(&path).map_err(|_| {
            Error::Invalid(format!(
                "cannot update the ref '{name}': there are still logs under '{}'",
                display_log_path(repo, &relative)
            ))
        })?;
    }
    let result = (|| -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().append(true).create(true).open(&path)?;
        file.write_all(record)?;
        file.sync_all()
    })();
    result.map_err(|error| {
        Error::Invalid(format!(
            "cannot update the ref '{name}': unable to append to '{}': {error}",
            display_log_path(repo, &relative)
        ))
    })
}

fn display_log_path(repo: &Repository, relative: &str) -> String {
    match &repo.worktree {
        Some(root) => repo
            .git_dir
            .strip_prefix(root)
            .map(|path| format!("{}/{relative}", path.display()))
            .unwrap_or_else(|_| repo.git_dir.join(relative).display().to_string()),
        None => repo.git_dir.join(relative).display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reflog_messages_collapse_ascii_whitespace_without_changing_other_bytes() {
        assert_eq!(
            normalize(b" \tfirst  second\nthird\r\n"),
            b"first second third"
        );
        assert_eq!(normalize(b"\xff  \xfe"), b"\xff \xfe");
        assert!(normalize(b" \n\t").is_empty());
    }
}

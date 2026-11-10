// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Small OS-facing primitives. Runtime Git commands cannot spawn child processes
//! in this milestone; process execution belongs exclusively to conformance tooling.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Resolve a GitHub credential without running Git, a credential helper, or any
/// other executable. macOS uses Security.framework directly; other platforms
/// currently accept an explicitly supplied environment credential.
pub fn github_token(account: &str) -> io::Result<String> {
    if let Some(token) = std::env::var_os("FERRAGIT_TOKEN") {
        return token.into_string().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "FERRAGIT_TOKEN is not UTF-8")
        });
    }
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::passwords::{
            find_internet_password, SecAuthenticationType, SecProtocolType,
        };
        let account =
            std::env::var("FERRAGIT_CREDENTIAL_ACCOUNT").unwrap_or_else(|_| account.to_owned());
        let (password,_) = find_internet_password(None,"github.com",None,&account,"",None,SecProtocolType::HTTPS,SecAuthenticationType::Default)
            .map_err(|_|io::Error::new(io::ErrorKind::PermissionDenied,"GitHub credential unavailable in macOS Keychain; set FERRAGIT_TOKEN or FERRAGIT_CREDENTIAL_ACCOUNT"))?;
        String::from_utf8(password.as_ref().to_vec()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "GitHub credential is not UTF-8")
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "set FERRAGIT_TOKEN; native credential storage is not implemented on this platform yet",
        ))
    }
}

/// A private, create-new temporary file in the destination directory. Keeping it
/// on the same filesystem allows installation without overwriting an existing object.
pub struct TempFile {
    path: PathBuf,
    file: Option<File>,
}

impl TempFile {
    pub fn in_directory(directory: &Path) -> io::Result<Self> {
        for _ in 0..100 {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!("tmp_obj_{}_{}", std::process::id(), sequence));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o444);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary filename exhaustion",
        ))
    }

    pub fn writer(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary file is open until consumed")
    }

    /// Publish complete content without replacing a competing writer's file.
    /// Both Unix and NTFS support hard links; unsupported filesystems fail closed.
    /// This syncs file contents, but directory crash durability remains a tracked gap.
    pub fn persist_no_clobber(mut self, destination: &Path) -> io::Result<()> {
        let file = self
            .file
            .take()
            .expect("temporary file is open until consumed");
        file.sync_all()?;
        drop(file); // Windows may refuse removal of an open file.
        match fs::hard_link(&self.path, destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub fn executable_file_mode() -> bool {
    cfg!(unix)
}

/// An exclusive Git-style lock file. A successful rename publishes the update;
/// every failure leaves the previous destination intact and removes our lock.
pub struct LockFile {
    path: PathBuf,
    file: Option<File>,
    committed: bool,
    prepared: bool,
}

impl LockFile {
    pub fn acquire(destination: &Path) -> io::Result<Self> {
        let mut lock = destination.as_os_str().to_os_string();
        lock.push(".lock");
        let path = PathBuf::from(lock);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
            committed: false,
            prepared: false,
        })
    }

    /// Finish and close the owned file without publishing its contents.
    pub fn prepare(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "lock is already closed"))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        self.prepared = true;
        Ok(())
    }

    pub fn publish(mut self, destination: &Path) -> io::Result<()> {
        if !self.prepared {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "lock contents are not prepared",
            ));
        }
        fs::rename(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }

    pub fn commit(mut self, destination: &Path, bytes: &[u8]) -> io::Result<()> {
        self.prepare(bytes)?;
        self.publish(destination)
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn path_bytes(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        path.to_str()
            .map(|s| s.replace('\\', "/").into_bytes())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "non-Unicode Windows path is not yet supported",
                )
            })
    }
}

pub fn path_from_bytes(bytes: &[u8]) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "non-UTF-8 Windows path"))
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn prepared_contents_are_private_until_publication_and_unprepared_locks_cannot_publish() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ferragit-lock-prepare-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("value");
        fs::write(&destination, b"original").unwrap();
        let lock = LockFile::acquire(&destination).unwrap();
        assert!(lock.publish(&destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert!(!directory.join("value.lock").exists());
        let mut lock = LockFile::acquire(&destination).unwrap();
        lock.prepare(b"replacement").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(
            fs::read(directory.join("value.lock")).unwrap(),
            b"replacement"
        );
        lock.publish(&destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert!(!directory.join("value.lock").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}

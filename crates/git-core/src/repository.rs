// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

mod packed_refs;
use crate::{
    config::Config,
    object::{HashKind, ObjectId},
    store::ObjectStore,
    Error, Result,
};
use git_platform::write_new;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Repository {
    pub git_dir: PathBuf,
    pub worktree: Option<PathBuf>,
    pub config: Config,
    pub objects: ObjectStore,
}

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

impl Repository {
    pub fn discover(start: &Path) -> Result<Self> {
        for key in [
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
        ] {
            if env::var_os(key).is_some() {
                return Err(Error::Unsupported(key.to_owned()));
            }
        }
        if let Some(path) = env::var_os("GIT_DIR") {
            let path = start.join(path);
            return Self::open(&path, Some(start.to_path_buf()));
        }
        let start = start.canonicalize()?;
        for directory in start.ancestors() {
            let git = directory.join(".git");
            if git.is_dir() {
                return Self::open(&git, Some(directory.to_path_buf()));
            }
            if git.is_file() {
                return Err(Error::Unsupported(
                    "gitdir files and linked worktrees".into(),
                ));
            }
            if directory.join("HEAD").is_file() && directory.join("objects").is_dir() {
                return Self::open(directory, None);
            }
        }
        Err(Error::NotRepository)
    }

    pub fn open(git_dir: &Path, worktree: Option<PathBuf>) -> Result<Self> {
        let config = Config::read(&git_dir.join("config"))?;
        let hash = config.repository_hash()?;
        if config.get("core.worktree").is_some() {
            return Err(Error::Unsupported("core.worktree".into()));
        }
        let bare = config.boolean("core.bare", false)?;
        Ok(Self {
            git_dir: git_dir.to_path_buf(),
            worktree: if bare { None } else { worktree },
            config,
            objects: ObjectStore::new(git_dir, hash),
        })
    }

    /// Initializes only new repositories. Reinitialization is refused rather than
    /// risking a rewrite of configuration/extensions that we cannot yet preserve.
    pub fn init(path: &Path, bare: bool, branch: &str, hash: HashKind) -> Result<Self> {
        let reference = format!("refs/heads/{branch}");
        if !valid_ref(&reference) || branch.starts_with('-') {
            return Err(Error::Invalid(format!(
                "invalid initial branch name: '{branch}'"
            )));
        }
        let git_dir = if bare {
            path.to_path_buf()
        } else {
            path.join(".git")
        };
        if !bare && git_dir.exists() {
            return Err(Error::Unsupported(
                "reinitializing an existing repository".into(),
            ));
        }
        fs::create_dir_all(path)?;
        let root = path.canonicalize()?;
        let git_dir = if bare {
            root.clone()
        } else {
            root.join(".git")
        };
        // A bare init targets an existing directory, so inspect Git-owned names
        // before creating any files. Never overwrite an existing config or HEAD.
        if git_dir.join("HEAD").exists() || git_dir.join("config").exists() {
            return Err(Error::Unsupported(
                "reinitializing an existing repository".into(),
            ));
        }
        for directory in ["objects/info", "objects/pack", "refs/heads", "refs/tags"] {
            fs::create_dir_all(git_dir.join(directory))?;
        }
        write_new(
            &git_dir.join("HEAD"),
            format!("ref: {reference}\n").as_bytes(),
        )?;
        let version = if hash == HashKind::Sha256 { 1 } else { 0 };
        let mut config = format!(
            "[core]\n\trepositoryformatversion = {version}\n\tfilemode = {}\n\tbare = {bare}\n",
            git_platform::executable_file_mode()
        );
        if !bare {
            config.push_str("\tlogallrefupdates = true\n");
        }
        if hash == HashKind::Sha256 {
            config.push_str("[extensions]\n\tobjectformat = sha256\n");
        }
        write_new(&git_dir.join("config"), config.as_bytes())?;
        Self::open(&git_dir, if bare { None } else { Some(root) })
    }

    pub fn resolve(&self, name: &str) -> Result<ObjectId> {
        if name.bytes().all(|b| b.is_ascii_hexdigit()) && name.len() >= 4 {
            return self.objects.resolve_hex(name);
        }
        let reference = if name == "HEAD" {
            "HEAD".to_owned()
        } else if valid_ref(name) {
            name.to_owned()
        } else {
            format!("refs/heads/{name}")
        };
        if reference != "HEAD" && !valid_ref(&reference) {
            return Err(Error::Invalid(format!("Not a valid object name {name}")));
        }
        self.read_ref(&reference, 0)?
            .ok_or_else(|| Error::Invalid(format!("Not a valid object name {name}")))
    }

    pub fn read_ref(&self, reference: &str, depth: u8) -> Result<Option<ObjectId>> {
        if depth > 8 || (reference != "HEAD" && !valid_ref(reference)) {
            return Err(Error::Invalid("invalid or recursive reference".into()));
        }
        let text = match fs::read_to_string(self.git_dir.join(reference)) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return packed_refs::lookup(
                    &self.git_dir.join("packed-refs"),
                    self.objects.hash,
                    reference,
                );
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(target) = text.trim().strip_prefix("ref: ") {
            return self.read_ref(target, depth + 1);
        }
        Ok(Some(ObjectId::parse(self.objects.hash, text.trim())?))
    }

    pub fn head_target(&self) -> Result<String> {
        let head = fs::read_to_string(self.git_dir.join("HEAD"))?;
        let target = head
            .trim()
            .strip_prefix("ref: ")
            .ok_or_else(|| Error::Unsupported("detached HEAD writes".into()))?;
        if !valid_ref(target) {
            return Err(Error::Invalid("invalid HEAD target".into()));
        }
        Ok(target.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_cannot_escape_the_repository() {
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

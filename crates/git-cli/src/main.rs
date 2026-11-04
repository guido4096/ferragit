// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Partial Git CLI. Unsupported commands never fall back to another executable.
use git_core::{
    commit,
    identity::Identity,
    index,
    object::{HashKind, Object, ObjectKind},
    push,
    refs::{Expected, Update},
    repository::Repository,
    store::{read_bounded, MAX_OBJECT_BYTES},
    Error, Result,
};
use std::{
    collections::{BTreeMap, VecDeque},
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct Args(VecDeque<OsString>);
impl Args {
    fn next(&mut self) -> Option<OsString> {
        self.0.pop_front()
    }
    fn value(&mut self, flag: &str) -> Result<OsString> {
        self.next()
            .ok_or_else(|| Error::Invalid(format!("option '{flag}' requires a value")))
    }
    fn text(&mut self, flag: &str) -> Result<String> {
        text(self.value(flag)?)
    }
    fn empty(&self) -> bool {
        self.0.is_empty()
    }
}
fn text(value: OsString) -> Result<String> {
    value
        .into_string()
        .map_err(|_| Error::Unsupported("non-Unicode command argument".into()))
}
fn output(bytes: &[u8]) -> Result<()> {
    io::stdout().lock().write_all(bytes)?;
    Ok(())
}
fn line(value: impl std::fmt::Display) -> Result<()> {
    output(format!("{value}\n").as_bytes())
}
fn unsupported(value: &str) -> Error {
    Error::Unsupported(value.to_owned())
}
const HELP:&str="ferragit: an independent, early Rust Git implementation\n\nUsage: ferragit [-C <directory>] [-c <key=value>] <command> [options]\n\nImplemented subsets: init, hash-object, cat-file, add, write-tree, commit,\n                      rev-parse, ls-files, update-ref, push, version\n\nSee README.md and compat/status.json for limits. No fallback to C Git.\n";

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(error) => {
            eprintln!("fatal: {error}");
            128
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let mut args = Args(env::args_os().skip(1).collect());
    let mut cwd = env::current_dir()?;
    let mut settings = BTreeMap::new();
    let command = loop {
        let Some(arg) = args.next() else {
            output(HELP.as_bytes())?;
            return Ok(1);
        };
        match arg.to_str() {
            Some("-C") => {
                cwd = cwd.join(args.value("-C")?);
                cwd = cwd.canonicalize()?;
            }
            Some("-c") => {
                let value = args.text("-c")?;
                let (key, value) = value
                    .split_once('=')
                    .ok_or_else(|| Error::Invalid("-c requires key=value".into()))?;
                settings.insert(key.to_ascii_lowercase(), value.to_owned());
            }
            Some("--version" | "-v") => {
                line("git version 2.56.0.ferragit.0.1.0")?;
                return Ok(0);
            }
            Some("--help" | "-h") => {
                output(HELP.as_bytes())?;
                return Ok(0);
            }
            Some(value) if value.starts_with('-') => return Err(unsupported(value)),
            _ => break text(arg)?,
        }
    };
    for key in settings.keys() {
        if !matches!(
            key.as_str(),
            "user.name" | "user.email" | "init.defaultbranch" | "advice.defaultbranchname"
        ) {
            return Err(unsupported(&format!("configuration override {key}")));
        }
    }
    match command.as_str() {
        "version" => {
            if !args.empty() {
                return Err(unsupported("version options"));
            }
            line("git version 2.56.0.ferragit.0.1.0")?;
        }
        "init" => init(&cwd, args, &settings)?,
        "hash-object" => hash_object(&cwd, args)?,
        "cat-file" => return cat_file(&cwd, args),
        "add" => {
            if args.next().as_deref() != Some(std::ffi::OsStr::new("-A")) || !args.empty() {
                return Err(unsupported(
                    "add currently requires exactly '-A' from the worktree root",
                ));
            }
            let repo = Repository::discover(&cwd)?;
            if repo.worktree.as_ref() != Some(&cwd.canonicalize()?) {
                return Err(unsupported("add -A outside worktree root"));
            }
            index::add_all(&repo)?;
        }
        "write-tree" => {
            if !args.empty() {
                return Err(unsupported("write-tree options"));
            }
            line(commit::write_tree(&Repository::discover(&cwd)?)?)?;
        }
        "commit" => create_commit(&cwd, args, &settings)?,
        "update-ref" => update_ref(&cwd, args, &settings)?,
        "rev-parse" => {
            let repo = Repository::discover(&cwd)?;
            let value = args.text("revision")?;
            if !args.empty() {
                return Err(unsupported("multiple rev-parse arguments"));
            }
            match value.as_str() {
                "--git-dir" => line(repo.git_dir.display())?,
                "--show-object-format" => line(repo.objects.hash.name())?,
                _ => line(repo.resolve(&value)?)?,
            }
        }
        "ls-files" => {
            let zero = match args.next() {
                None => false,
                Some(arg) if arg == "-z" => true,
                _ => return Err(unsupported("ls-files options")),
            };
            if !args.empty() {
                return Err(unsupported("ls-files arguments"));
            }
            for entry in index::read(&Repository::discover(&cwd)?)? {
                if !zero
                    && entry
                        .path
                        .iter()
                        .any(|b| *b < 32 || *b >= 127 || *b == b'"' || *b == b'\\')
                {
                    return Err(unsupported("quoted ls-files paths; use -z"));
                }
                output(&entry.path)?;
                output(if zero { b"\0" } else { b"\n" })?;
            }
        }
        "repo" => {
            let action = args.text("repo action")?;
            if action == "api" || action == "download" {
                let mut name = args.text("repository")?;
                let anonymous = name == "--anonymous";
                if anonymous {
                    name = args.text("repository")?;
                }
                let resource = args.text("API resource")?;
                let destination = if action == "download" {
                    Some(PathBuf::from(args.value("destination")?))
                } else {
                    None
                };
                if !args.empty() {
                    return Err(unsupported("repository evidence arguments"));
                }
                let (owner, _) = git_core::github::validate_name(&name)?;
                let token = if anonymous {
                    String::new()
                } else {
                    git_platform::github_token(owner)?
                };
                return match destination {
                    Some(path) => {
                        line(git_core::github::download(&name, &resource, &path, &token)?)?;
                        Ok(0)
                    }
                    None => {
                        line(git_core::github::api(&name, &resource, &token)?)?;
                        Ok(0)
                    }
                };
            }

            if !matches!(
                action.as_str(),
                "create" | "inspect" | "verify" | "rename" | "checks" | "job-log"
            ) {
                return Err(unsupported("repo action"));
            }
            if action == "create" && args.text("visibility")? != "--private" {
                return Err(unsupported("use: repo create --private OWNER/NAME"));
            }
            let name = args.text("repository")?;
            let extra = if action == "rename" || action == "job-log" {
                args.text("repository name or job ID")?
            } else {
                String::new()
            };
            if !args.empty() {
                return Err(unsupported("repo arguments"));
            }
            let job = if action == "job-log" {
                extra
                    .parse::<u64>()
                    .map_err(|_| Error::Invalid("job ID must be an unsigned integer".into()))?
            } else {
                0
            };
            let (owner, _) = git_core::github::validate_name(&name)?;
            let token = git_platform::github_token(owner)?;
            match action.as_str() {
                "create" => line(git_core::github::create_private(&name, &token)?)?,
                "inspect" => line(git_core::github::inspect(&name, &token)?)?,
                "rename" => line(git_core::github::rename(&name, &extra, &token)?)?,
                "job-log" => output(&git_core::github::job_log(&name, job, &token)?)?,
                "checks" => line(git_core::github::checks(
                    &Repository::discover(&cwd)?,
                    &name,
                    &token,
                )?)?,
                "verify" => line(git_core::github::verify(
                    &Repository::discover(&cwd)?,
                    &name,
                    &token,
                )?)?,
                _ => return Err(unsupported("repo action")),
            }
        }
        "push" => {
            let remote = args.text("remote URL")?;
            let refspec = args.text("refspec")?;
            if !args.empty() {
                return Err(unsupported("push options"));
            }
            let (source, destination) = refspec.split_once(':').unwrap_or((&refspec, &refspec));
            let destination = if destination.starts_with("refs/") {
                destination.to_owned()
            } else {
                format!("refs/heads/{destination}")
            };
            let owner = remote
                .strip_prefix("https://github.com/")
                .and_then(|path| path.split('/').next())
                .ok_or_else(|| unsupported("remote URL"))?;
            let token = git_platform::github_token(owner)?;
            let id = push::https(
                &Repository::discover(&cwd)?,
                &remote,
                source,
                &destination,
                &token,
            )?;
            eprintln!("To {remote}\n   {id} -> {destination}");
        }
        _ => return Err(unsupported(&format!("command '{command}'"))),
    }
    Ok(0)
}

fn init(cwd: &Path, mut args: Args, settings: &BTreeMap<String, String>) -> Result<()> {
    let mut bare = false;
    let mut quiet = false;
    let mut branch = settings
        .get("init.defaultbranch")
        .cloned()
        .unwrap_or_else(|| "master".into());
    let mut hash = env::var("GIT_DEFAULT_HASH")
        .ok()
        .map(|s| HashKind::parse(&s))
        .transpose()?
        .unwrap_or(HashKind::Sha1);
    let mut directory = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--bare") => bare = true,
            Some("-q" | "--quiet") => quiet = true,
            Some("-b" | "--initial-branch") => branch = args.text("--initial-branch")?,
            Some("--object-format") => hash = HashKind::parse(&args.text("--object-format")?)?,
            Some("--template") => {
                if !args.value("--template")?.is_empty() {
                    return Err(unsupported("nonempty init templates"));
                }
            }
            Some(value) if value.starts_with("--object-format=") => {
                hash = HashKind::parse(&value[16..])?
            }
            Some(value) if value.starts_with("--initial-branch=") => {
                branch = value[17..].to_owned()
            }
            Some("--template=") => {}
            Some(value) if value.starts_with('-') => return Err(unsupported(value)),
            _ => {
                if directory.replace(PathBuf::from(arg)).is_some() {
                    return Err(Error::Invalid("too many init directories".into()));
                }
            }
        }
    }
    let repo = Repository::init(
        &cwd.join(directory.unwrap_or_default()),
        bare,
        &branch,
        hash,
    )?;
    if !quiet {
        line(format!(
            "Initialized empty Git repository in {}/",
            repo.git_dir.display()
        ))?;
    }
    Ok(())
}

fn hash_object(cwd: &Path, mut args: Args) -> Result<()> {
    let mut stdin = false;
    let mut write = false;
    let mut kind = ObjectKind::Blob;
    let mut literal = false;
    let mut no_filters = false;
    let mut paths = Vec::new();
    let mut end = false;
    while let Some(arg) = args.next() {
        if end {
            paths.push(PathBuf::from(arg));
            continue;
        }
        match arg.to_str() {
            Some("--") => end = true,
            Some("--stdin") => stdin = true,
            Some("-w") => write = true,
            Some("-t") => kind = ObjectKind::parse(&args.text("-t")?)?,
            Some("--literally") => literal = true,
            Some("--no-filters") => no_filters = true,
            Some(value) if value.starts_with('-') => return Err(unsupported(value)),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if !stdin && paths.is_empty() {
        return Err(Error::Invalid(
            "hash-object requires --stdin or file paths".into(),
        ));
    }
    if stdin && !paths.is_empty() {
        return Err(unsupported("combined stdin and file hashing"));
    }
    if kind != ObjectKind::Blob && !literal {
        return Err(unsupported(
            "non-blob validation; --literally is required for raw objects",
        ));
    }
    let repo = match Repository::discover(cwd) {
        Ok(repo) => Some(repo),
        Err(Error::NotRepository) if !write => None,
        Err(error) => return Err(error),
    };
    let hash = repo
        .as_ref()
        .map(|r| r.objects.hash)
        .unwrap_or(HashKind::Sha1);
    let emit = |bytes| -> Result<()> {
        let object = Object { kind, data: bytes };
        let id = if write {
            repo.as_ref().unwrap().objects.write(&object)?
        } else {
            hash.digest(&object.encode())?
        };
        line(id)
    };
    if stdin {
        emit(read_bounded(io::stdin().lock(), MAX_OBJECT_BYTES)?)?;
    }
    if !paths.is_empty() && !no_filters {
        return Err(unsupported(
            "file hashing without --no-filters; attribute conversions are not implemented",
        ));
    }
    for path in paths {
        emit(read_bounded(
            fs::File::open(cwd.join(path))?,
            MAX_OBJECT_BYTES,
        )?)?;
    }
    Ok(())
}

fn cat_file(cwd: &Path, mut args: Args) -> Result<i32> {
    let mode = args.text("cat-file mode")?;
    let name = args.text("object")?;
    if !args.empty() {
        return Err(unsupported("cat-file arguments"));
    }
    let repo = Repository::discover(cwd)?;
    let object = repo.resolve(&name).and_then(|id| repo.objects.read(&id));
    let object = match object {
        Ok(object) => object,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            if mode == "-e" {
                return Ok(1);
            }
            return Err(Error::Invalid(
                "git cat-file: could not get object info".into(),
            ));
        }
        Err(error) => return Err(error),
    };
    match mode.as_str() {
        "-e" => {}
        "-t" => line(object.kind.name())?,
        "-s" => line(object.data.len())?,
        "-p" if object.kind != ObjectKind::Tree => output(&object.data)?,
        "blob" | "tree" | "commit" | "tag" => {
            if ObjectKind::parse(&mode)? != object.kind {
                return Err(Error::Invalid(format!("git cat-file {name}: bad file")));
            }
            output(&object.data)?;
        }
        _ => {
            return Err(unsupported(
                "cat-file mode (tree pretty-print and batch modes are pending)",
            ))
        }
    }
    Ok(0)
}

fn create_commit(cwd: &Path, mut args: Args, settings: &BTreeMap<String, String>) -> Result<()> {
    let mut message = None;
    let mut quiet = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-m" | "--message") => {
                if message.replace(args.text("-m")?.into_bytes()).is_some() {
                    return Err(unsupported("multiple commit messages"));
                }
            }
            Some("-F" | "--file") => {
                if message
                    .replace(read_bounded(
                        fs::File::open(cwd.join(args.value("-F")?))?,
                        MAX_OBJECT_BYTES,
                    )?)
                    .is_some()
                {
                    return Err(unsupported("multiple commit messages"));
                }
            }
            Some("-q" | "--quiet") => quiet = true,
            _ => return Err(unsupported("commit option")),
        }
    }
    let repo = Repository::discover(cwd)?;
    let message = message.ok_or_else(|| unsupported("interactive commit editor; use -m or -F"))?;
    let id = commit::create(
        &repo,
        &message,
        &identity(&repo, settings, "AUTHOR")?,
        &identity(&repo, settings, "COMMITTER")?,
    )?;
    if !quiet {
        line(format!(
            "[{} {}] {}",
            repo.head_target()?.trim_start_matches("refs/heads/"),
            &id.to_string()[..7],
            String::from_utf8_lossy(&message)
                .lines()
                .next()
                .unwrap_or("")
        ))?;
    }
    Ok(())
}

fn identity(
    repo: &Repository,
    settings: &BTreeMap<String, String>,
    role: &str,
) -> Result<Identity> {
    let setting = |key: &str| {
        settings
            .get(key)
            .map(String::as_str)
            .or_else(|| repo.config.get(key))
            .map(str::to_owned)
    };
    let default_name = setting("user.name");
    let default_email = setting("user.email");
    let date = format!(
        "{} +0000",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid("system clock precedes epoch".into()))?
            .as_secs()
    );
    Ok(Identity {
        name: env::var(format!("GIT_{role}_NAME"))
            .ok()
            .or_else(|| default_name.clone())
            .ok_or_else(|| {
                Error::Invalid("Author identity unknown; set user.name or GIT_AUTHOR_NAME".into())
            })?,
        email: env::var(format!("GIT_{role}_EMAIL"))
            .ok()
            .or_else(|| default_email.clone())
            .ok_or_else(|| {
                Error::Invalid("Author identity unknown; set user.email or GIT_AUTHOR_EMAIL".into())
            })?,
        date: env::var(format!("GIT_{role}_DATE"))
            .unwrap_or_else(|_| date.clone())
            .trim_start_matches('@')
            .to_owned(),
    })
}

fn update_ref(cwd: &Path, mut args: Args, settings: &BTreeMap<String, String>) -> Result<()> {
    let mut reason = Vec::new();
    let reference = loop {
        let value = args.text("reference")?;
        match value.as_str() {
            "-m" => reason = args.text("reflog message")?.into_bytes(),
            "--" => break args.text("reference")?,
            value if value.starts_with('-') => return Err(unsupported("update-ref option")),
            _ => break value,
        }
    };
    let new = args.text("new object")?;
    let old = args.next().map(text).transpose()?;
    if !args.empty() {
        return Err(unsupported("update-ref arguments"));
    }
    let repo = Repository::discover(cwd)?;
    let oid = |value: &str| {
        if value.len() == repo.objects.hash.hex_len()
            && value.bytes().all(|b| b.is_ascii_hexdigit())
        {
            git_core::object::ObjectId::parse(repo.objects.hash, value)
        } else {
            repo.resolve(value)
        }
    };
    let new = oid(&new)?;
    let expected = match old.as_deref() {
        None => Expected::Any,
        Some(value)
            if value.is_empty()
                || value.len() == repo.objects.hash.hex_len()
                    && value.bytes().all(|b| b == b'0') =>
        {
            Expected::Missing
        }
        Some(value) => Expected::Value(oid(value)?),
    };
    Update::lock(&repo, &reference, expected)
        .and_then(|update| update.finish(&new, || identity(&repo, settings, "COMMITTER"), &reason))
        .map_err(|error| {
            Error::Invalid(format!("update_ref failed for ref '{reference}': {error}"))
        })
}

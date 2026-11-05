// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Executable comparisons for the milestone; not the complete upstream suite.
use git_core::{
    object::{HashKind, ObjectId},
    push,
    repository::Repository,
};
use serde::Serialize;
use std::{
    env,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
static NEXT: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
    pub path: PathBuf,
    keep: bool,
}
impl Fixture {
    pub fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "ferragit-conformance-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, keep: false }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "fixture names exhausted",
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Serialize)]
pub struct Case {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}
#[derive(Serialize)]
pub struct Report {
    pub schema: u32,
    pub platform: String,
    pub architecture: String,
    pub reference: String,
    pub candidate: String,
    pub reference_version: String,
    pub reference_build_options: String,
    pub candidate_version: String,
    pub scope: String,
    pub process_guard: Option<Vec<String>>,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<Case>,
    pub retained_fixture: Option<String>,
}

pub fn executable(name: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        return path.canonicalize();
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(name);
        if path.is_file() {
            return path.canonicalize();
        }
        #[cfg(windows)]
        {
            let path = path.with_extension("exe");
            if path.is_file() {
                return path.canonicalize();
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("executable not found: {}", name.to_string_lossy()),
    ))
}

fn process_guard() -> io::Result<Option<Vec<String>>> {
    let Some(value) = env::var_os("FERRAGIT_PROCESS_GUARD") else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| io::Error::other("invalid guard command"))?;
    let prefix: Vec<String> = serde_json::from_str(&value).map_err(io::Error::other)?;
    if prefix.is_empty() || !Path::new(&prefix[0]).is_absolute() {
        return Err(io::Error::other(
            "guard command must have an absolute executable",
        ));
    }
    Ok(Some(prefix))
}

pub fn invoke(
    binary: &Path,
    cwd: &Path,
    home: &Path,
    args: &[&str],
    input: &[u8],
    candidate: bool,
) -> io::Result<Output> {
    let guard = if candidate { process_guard()? } else { None };
    let mut command = if let Some(prefix) = guard {
        let mut command = Command::new(&prefix[0]);
        command.args(&prefix[1..]).arg(binary);
        command
    } else {
        Command::new(binary)
    };
    command.args(args).current_dir(cwd).env_clear();
    for key in [
        "SYSTEMROOT",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TMP",
        "TEMP",
        "TMPDIR",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env(
        "PATH",
        if candidate {
            home.as_os_str().to_owned()
        } else {
            env::var_os("PATH").unwrap_or_default()
        },
    );
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("empty-config"))
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .env("GIT_AUTHOR_NAME", "Conformance Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.invalid")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_NAME", "Conformance Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.invalid")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000");
    if candidate {
        command.env("GIT_EXEC_PATH", home);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    // Concurrent input writing lets wait_with_output drain output while the child reads.
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    if let Err(error) = writer
        .join()
        .map_err(|_| io::Error::other("stdin writer panicked"))?
    {
        if error.kind() != io::ErrorKind::BrokenPipe {
            return Err(error);
        }
    }
    Ok(output)
}
fn equivalent(left: &Output, right: &Output) -> bool {
    left.status == right.status && left.stdout == right.stdout && left.stderr == right.stderr
}
fn describe(output: &Output) -> String {
    format!(
        "exit={:?}, stdout={:?}, stderr={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn checked(output: Output) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(describe(&output).into())
    }
}

struct Suite {
    reference: PathBuf,
    candidate: PathBuf,
    home: PathBuf,
    cases: Vec<Case>,
}
impl Suite {
    fn record(&mut self, name: impl Into<String>, passed: bool, detail: impl Into<String>) {
        self.cases.push(Case {
            name: name.into(),
            passed,
            detail: detail.into(),
        });
    }
    fn compare(
        &mut self,
        name: &str,
        left: &Path,
        right: &Path,
        args: &[&str],
        input: &[u8],
    ) -> Result<Output> {
        let reference = invoke(&self.reference, left, &self.home, args, input, false)?;
        let candidate = invoke(&self.candidate, right, &self.home, args, input, true)?;
        let same = equivalent(&reference, &candidate);
        self.record(
            name,
            same,
            if same {
                String::new()
            } else {
                format!(
                    "reference: {}; candidate: {}; argv={args:?}",
                    describe(&reference),
                    describe(&candidate)
                )
            },
        );
        Ok(candidate)
    }
    fn oracle(&self, cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output> {
        Ok(invoke(
            &self.reference,
            cwd,
            &self.home,
            args,
            input,
            false,
        )?)
    }
    fn candidate(&self, cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output> {
        Ok(invoke(&self.candidate, cwd, &self.home, args, input, true)?)
    }

    fn hash_suite(&mut self, root: &Path, algorithm: &str) -> Result<()> {
        let left = root.join(format!("{algorithm}-reference"));
        let right = root.join(format!("{algorithm}-candidate"));
        fs::create_dir(&left)?;
        fs::create_dir(&right)?;
        let format = format!("--object-format={algorithm}");
        self.compare(
            &format!("{algorithm}/init"),
            &left,
            &right,
            &["init", "-q", "--template=", "-b", "main", &format],
            b"",
        )?;
        self.record(
            format!("{algorithm}/HEAD-state"),
            fs::read(left.join(".git/HEAD"))? == fs::read(right.join(".git/HEAD"))?,
            "",
        );
        let actual = checked(self.oracle(&right, &["rev-parse", "--show-object-format"], b"")?)?;
        self.record(
            format!("{algorithm}/Git-opens-candidate"),
            actual.stdout == format!("{algorithm}\n").as_bytes(),
            describe(&actual),
        );
        for (i, data) in [
            vec![],
            b"hello\n".to_vec(),
            b"\0\xff\xfe\r\nbinary\0".to_vec(),
            (0..8193).map(|n| (n % 251) as u8).collect(),
        ]
        .iter()
        .enumerate()
        {
            self.compare(
                &format!("{algorithm}/hash-{i}"),
                &left,
                &right,
                &["hash-object", "--stdin"],
                data,
            )?;
            let result = checked(self.compare(
                &format!("{algorithm}/write-{i}"),
                &left,
                &right,
                &["hash-object", "-w", "--stdin"],
                data,
            )?)?;
            let id = std::str::from_utf8(&result.stdout)?.trim();
            for mode in ["-t", "-s", "-p", "-e", "blob"] {
                self.compare(
                    &format!("{algorithm}/cat-{i}-{mode}"),
                    &left,
                    &right,
                    &["cat-file", mode, id],
                    b"",
                )?;
            }
            self.compare(
                &format!("{algorithm}/abbreviated-{i}"),
                &left,
                &right,
                &["cat-file", "-t", &id[..8]],
                b"",
            )?;
            let read = checked(self.oracle(&right, &["cat-file", "blob", id], b"")?)?;
            self.record(
                format!("{algorithm}/Git-reads-Rust-object-{i}"),
                read.stdout == *data,
                "",
            );
            let extra = format!("oracle-only-{algorithm}-{i}");
            let created = checked(self.oracle(
                &left,
                &["hash-object", "-w", "--stdin"],
                extra.as_bytes(),
            )?)?;
            let extra_id = std::str::from_utf8(&created.stdout)?.trim();
            let suffix = format!(".git/objects/{}/{}", &extra_id[..2], &extra_id[2..]);
            fs::create_dir_all(right.join(&suffix).parent().unwrap())?;
            fs::copy(left.join(&suffix), right.join(&suffix))?;
            let read = checked(self.candidate(&right, &["cat-file", "blob", extra_id], b"")?)?;
            self.record(
                format!("{algorithm}/Rust-reads-Git-object-{i}"),
                read.stdout == extra.as_bytes(),
                "",
            );
        }
        let missing = "0".repeat(if algorithm == "sha1" { 40 } else { 64 });
        for mode in ["-e", "-t"] {
            self.compare(
                &format!("{algorithm}/missing-{mode}"),
                &left,
                &right,
                &["cat-file", mode, &missing],
                b"",
            )?;
        }
        self.workflow(root, &left, &right, algorithm)?;
        Ok(())
    }

    fn workflow(&mut self, root: &Path, left: &Path, right: &Path, algorithm: &str) -> Result<()> {
        for directory in [left, right] {
            fs::create_dir(directory.join("a"))?;
            fs::create_dir(directory.join("ignored"))?;
            fs::write(directory.join(".gitignore"), b"ignored/\n*.tmp\n")?;
            fs::write(directory.join("hello.txt"), b"hello\n")?;
            fs::write(directory.join("binary.bin"), b"\0\xff\r\n\0")?;
            fs::write(directory.join("a/inside"), b"nested\n")?;
            fs::write(directory.join("a.txt"), b"tree sort edge\n")?;
            fs::write(directory.join("ignored/secret"), b"must not stage")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::{symlink, PermissionsExt};
                fs::set_permissions(
                    directory.join("hello.txt"),
                    fs::Permissions::from_mode(0o755),
                )?;
                symlink("hello.txt", directory.join("link"))?;
            }
        }
        self.compare(
            &format!("{algorithm}/add"),
            left,
            right,
            &["add", "-A"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/ls-files"),
            left,
            right,
            &["ls-files", "-z"],
            b"",
        )?;
        let left_stage = checked(self.oracle(left, &["ls-files", "--stage", "-z"], b"")?)?;
        let right_stage = checked(self.oracle(right, &["ls-files", "--stage", "-z"], b"")?)?;
        self.record(
            format!("{algorithm}/index-state"),
            left_stage.stdout == right_stage.stdout,
            "",
        );
        self.compare(
            &format!("{algorithm}/tree"),
            left,
            right,
            &["write-tree"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/initial-commit"),
            left,
            right,
            &["commit", "-q", "-m", "Initial fixture"],
            b"",
        )?;
        let first = checked(self.compare(
            &format!("{algorithm}/commit-id"),
            left,
            right,
            &["rev-parse", "HEAD"],
            b"",
        )?)?;
        let first = String::from_utf8(first.stdout)?.trim().to_owned();
        self.compare(
            &format!("{algorithm}/commit-bytes"),
            left,
            right,
            &["cat-file", "commit", "HEAD"],
            b"",
        )?;
        let status = checked(self.oracle(right, &["status", "--porcelain=v1", "-z"], b"")?)?;
        self.record(
            format!("{algorithm}/Git-sees-clean-worktree"),
            status.stdout.is_empty(),
            describe(&status),
        );
        for directory in [left, right] {
            fs::write(directory.join("hello.txt"), b"changed\n")?;
            fs::remove_file(directory.join("binary.bin"))?;
            fs::write(directory.join("new.txt"), b"new\n")?;
        }
        self.compare(
            &format!("{algorithm}/add-changes"),
            left,
            right,
            &["add", "-A"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/second-commit"),
            left,
            right,
            &["commit", "-q", "-m", "Update fixture"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/second-id"),
            left,
            right,
            &["rev-parse", "HEAD"],
            b"",
        )?;
        let fsck = self.oracle(right, &["fsck", "--strict", "--no-dangling"], b"")?;
        self.record(
            format!("{algorithm}/Git-fsck"),
            fsck.status.success(),
            describe(&fsck),
        );

        // Repack both independent fixtures with the oracle. The candidate must
        // keep reading and committing without any loose copy of its old HEAD.
        let packed_head = Repository::discover(right)?.resolve("HEAD")?.to_string();
        for directory in [left, right] {
            checked(self.oracle(directory, &["repack", "-a", "-d"], b"")?)?;
            checked(self.oracle(directory, &["pack-refs", "--all"], b"")?)?;
        }
        self.record(
            format!("{algorithm}/HEAD-is-packed"),
            !right
                .join(format!(
                    ".git/objects/{}/{}",
                    &packed_head[..2],
                    &packed_head[2..]
                ))
                .exists()
                && !right.join(".git/refs/heads/main").exists(),
            "",
        );
        self.compare(
            &format!("{algorithm}/packed-HEAD"),
            left,
            right,
            &["rev-parse", "HEAD"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/packed-commit-bytes"),
            left,
            right,
            &["cat-file", "commit", "HEAD"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/packed-abbreviation"),
            left,
            right,
            &["cat-file", "-t", &packed_head[..8]],
            b"",
        )?;
        for directory in [left, right] {
            fs::write(
                directory.join("after-repack.txt"),
                b"continue packed history\n",
            )?;
        }
        self.compare(
            &format!("{algorithm}/add-after-repack"),
            left,
            right,
            &["add", "-A"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/commit-after-repack"),
            left,
            right,
            &["commit", "-q", "-m", "Continue packed history"],
            b"",
        )?;
        self.compare(
            &format!("{algorithm}/mixed-storage-commit-id"),
            left,
            right,
            &["rev-parse", "HEAD"],
            b"",
        )?;
        let fsck = self.oracle(right, &["fsck", "--strict", "--no-dangling"], b"")?;
        self.record(
            format!("{algorithm}/Git-fsck-after-repack"),
            fsck.status.success(),
            describe(&fsck),
        );

        let head = Repository::discover(right)?.resolve("HEAD")?.to_string();
        let zero = "0".repeat(if algorithm == "sha1" { 40 } else { 64 });
        for (label, args) in [
            (
                "create-ref",
                vec![
                    "update-ref",
                    "-m",
                    "create topic",
                    "refs/heads/topic",
                    &first,
                    &zero,
                ],
            ),
            (
                "advance-ref",
                vec![
                    "update-ref",
                    "-m",
                    "advance topic",
                    "refs/heads/topic",
                    &head,
                    &first,
                ],
            ),
            (
                "ref-old-mismatch",
                vec!["update-ref", "refs/heads/topic", &first, &first],
            ),
            ("unchanged-HEAD", vec!["update-ref", "HEAD", &head, &head]),
        ] {
            self.compare(&format!("{algorithm}/{label}"), left, right, &args, b"")?;
        }
        for name in ["HEAD", "refs/heads/main", "refs/heads/topic"] {
            self.record(
                format!("{algorithm}/reflog-{name}"),
                fs::read(left.join(".git/logs").join(name))?
                    == fs::read(right.join(".git/logs").join(name))?,
                "",
            );
        }

        for directory in [left, right] {
            checked(self.oracle(directory, &["pack-refs", "--all"], b"")?)?;
        }
        for (label, args) in [
            (
                "override-packed-topic",
                vec!["update-ref", "refs/heads/topic", &first, &head],
            ),
            (
                "delete-old-mismatch",
                vec!["update-ref", "-d", "refs/heads/topic", &head],
            ),
            (
                "delete-packed-override",
                vec!["update-ref", "-d", "refs/heads/topic", &first],
            ),
            (
                "delete-missing-ref",
                vec!["update-ref", "-d", "refs/heads/topic"],
            ),
            (
                "create-zero-target",
                vec!["update-ref", "refs/heads/zero", &first, &zero],
            ),
            (
                "delete-zero-new",
                vec!["update-ref", "refs/heads/zero", &zero, &first],
            ),
        ] {
            self.compare(&format!("{algorithm}/{label}"), left, right, &args, b"")?;
        }
        self.record(
            format!("{algorithm}/delete-packed-state"),
            fs::read(left.join(".git/packed-refs"))? == fs::read(right.join(".git/packed-refs"))?
                && Repository::discover(right)?
                    .read_ref("refs/heads/topic", 0)?
                    .is_none()
                && !right.join(".git/logs/refs/heads/topic").exists(),
            "",
        );

        // All sender bytes are built by Rust. C Git only supplies the receiving
        // endpoint used to validate the protocol and independently inspect storage.
        let receiver = root.join(format!("{algorithm}-receiver"));
        fs::create_dir(&receiver)?;
        checked(self.oracle(
            &receiver,
            &[
                "init",
                "-q",
                "--bare",
                "--template=",
                &format!("--object-format={algorithm}"),
            ],
            b"",
        )?)?;
        let advertised = checked(self.oracle(
            &receiver,
            &["receive-pack", "--stateless-rpc", "--advertise-refs", "."],
            b"",
        )?)?;
        let mut advertisement = push::packet(b"# service=git-receive-pack\n")?;
        advertisement.extend(b"0000");
        advertisement.extend(&advertised.stdout);
        let hash = HashKind::parse(algorithm)?;
        let advertisement = push::Advertisement::parse(&advertisement, hash)?;
        let repo = Repository::discover(right)?;
        let head = repo.resolve("HEAD")?;
        let request = push::request(&repo, &head, "refs/heads/main", &advertisement)?;
        let received = checked(self.oracle(
            &receiver,
            &["receive-pack", "--stateless-rpc", "."],
            &request,
        )?)?;
        self.record(
            format!("{algorithm}/receive-pack-accepts-Rust-request"),
            push::check_status(&received.stdout, "refs/heads/main").is_ok(),
            describe(&received),
        );
        let remote = checked(self.oracle(&receiver, &["rev-parse", "refs/heads/main"], b"")?)?;
        self.record(
            format!("{algorithm}/remote-head"),
            remote.stdout == format!("{head}\n").as_bytes(),
            describe(&remote),
        );
        let fsck = self.oracle(&receiver, &["fsck", "--strict", "--no-dangling"], b"")?;
        self.record(
            format!("{algorithm}/remote-fsck"),
            fsck.status.success(),
            describe(&fsck),
        );
        let mut changed = advertisement;
        changed.refs.insert(
            "refs/heads/main".into(),
            ObjectId::parse(hash, &"a".repeat(hash.hex_len()))?,
        );
        self.record(
            format!("{algorithm}/non-fast-forward-rejected"),
            push::request(&repo, &head, "refs/heads/main", &changed).is_err(),
            "",
        );
        self.record(
            format!("{algorithm}/ancestor-push-accepted"),
            push::pack(&repo, &head, Some(&ObjectId::parse(hash, &first)?)).is_ok(),
            "",
        );
        Ok(())
    }
}

pub fn run_suite(candidate: &Path, reference: &Path, keep_failures: bool) -> Result<Report> {
    let candidate = executable(candidate.as_os_str())?;
    let reference = executable(reference.as_os_str())?;
    let mut fixture = Fixture::new()?;
    let home = fixture.path.join("home");
    fs::create_dir(&home)?;
    fs::write(home.join("empty-config"), b"")?;
    let reference_version = checked(invoke(
        &reference,
        &fixture.path,
        &home,
        &["--version"],
        b"",
        false,
    )?)?;
    let reference_build = checked(invoke(
        &reference,
        &fixture.path,
        &home,
        &["version", "--build-options"],
        b"",
        false,
    )?)?;
    let candidate_version = checked(invoke(
        &candidate,
        &fixture.path,
        &home,
        &["--version"],
        b"",
        true,
    )?)?;
    let mut suite = Suite {
        reference: reference.clone(),
        candidate: candidate.clone(),
        home,
        cases: Vec::new(),
    };
    suite.compare(
        "standalone-hash",
        &fixture.path,
        &fixture.path,
        &["hash-object", "--stdin"],
        b"outside repository\0",
    )?;
    for algorithm in ["sha1", "sha256"] {
        suite.hash_suite(&fixture.path, algorithm)?;
    }
    let unknown = suite.candidate(&fixture.path, &["not-an-installed-helper"], b"")?;
    suite.record(
        "unknown-command-fails-with-empty-exec-path",
        !unknown.status.success() && unknown.stderr.windows(11).any(|w| w == b"unsupported"),
        describe(&unknown),
    );
    let passed = suite.cases.iter().filter(|case| case.passed).count();
    let failed = suite.cases.len() - passed;
    fixture.keep = failed > 0 && keep_failures;
    Ok(Report {
        schema: 1,
        platform: env::consts::OS.into(),
        architecture: env::consts::ARCH.into(),
        reference: reference.display().to_string(),
        candidate: candidate.display().to_string(),
        reference_version: String::from_utf8(reference_version.stdout)?.trim().into(),
        reference_build_options: String::from_utf8(reference_build.stdout)?,
        candidate_version: String::from_utf8(candidate_version.stdout)?.trim().into(),
        process_guard: process_guard()?,
        scope: "focused loose/packed-object, reference-update/reflog, index, commit and pack interoperability; not full Git conformance"
            .into(),
        total: suite.cases.len(),
        passed,
        failed,
        cases: suite.cases,
        retained_fixture: if fixture.keep {
            Some(fixture.path.display().to_string())
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparator_detects_output_channel_line_ending_and_exit_changes() {
        let fixture = Fixture::new().unwrap();
        fs::write(fixture.path.join("empty-config"), b"").unwrap();
        let git = executable(OsStr::new("git")).unwrap();
        let baseline = invoke(
            &git,
            &fixture.path,
            &fixture.path,
            &["--version"],
            b"",
            false,
        )
        .unwrap();
        let mut changed = baseline.clone();
        changed.stderr = changed.stdout.clone();
        changed.stdout.clear();
        assert!(!equivalent(&baseline, &changed));
        let mut changed = baseline.clone();
        changed.stdout.extend(b"\r");
        assert!(!equivalent(&baseline, &changed));
        let mut changed = baseline.clone();
        changed.status = invoke(
            &git,
            &fixture.path,
            &fixture.path,
            &["nonesuch-command"],
            b"",
            false,
        )
        .unwrap()
        .status;
        assert!(!equivalent(&baseline, &changed));
    }
}

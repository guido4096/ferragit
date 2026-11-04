# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Qualify byte/state comparison and process isolation against a pinned oracle."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
from guard import launcher


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n")


def snapshot(directory):
    """No timestamps or inode numbers: those vary across equivalent fixtures."""
    entries = []
    for path in sorted(directory.rglob("*")):
        metadata = path.lstat()
        item = {"path": os.fsencode(path.relative_to(directory)).hex()}
        if stat.S_ISLNK(metadata.st_mode):
            item.update(kind="symlink", target=os.fsencode(os.readlink(path)).hex())
        elif stat.S_ISDIR(metadata.st_mode):
            item.update(kind="directory")
        elif stat.S_ISREG(metadata.st_mode):
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            item.update(kind="file", size=metadata.st_size, sha256=digest)
            if os.name != "nt":
                item["executable"] = bool(metadata.st_mode & 0o111)
        else:
            raise RuntimeError(f"unsupported fixture file type: {path}")
        entries.append(item)
    return entries


def controlled_environment(home):
    result = {key: value for key, value in os.environ.items() if key in
              ("PATH", "SYSTEMROOT", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT",
               "TMP", "TEMP", "TMPDIR")}
    result.update(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(home),
                  GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=str(home / "empty-config"),
                  GIT_CONFIG_COUNT="0", GIT_ATTR_NOSYSTEM="1", GIT_TERMINAL_PROMPT="0",
                  LC_ALL="C", LANG="C", TZ="UTC",
                  GIT_AUTHOR_NAME="Conformance Author", GIT_AUTHOR_EMAIL="author@example.invalid",
                  GIT_AUTHOR_DATE="1700000000 +0000", GIT_COMMITTER_NAME="Conformance Committer",
                  GIT_COMMITTER_EMAIL="committer@example.invalid", GIT_COMMITTER_DATE="1700000000 +0000")
    return result


def equivalent(left, right):
    return all(left[key] == right[key] for key in ("returncode", "stdout", "stderr", "state"))


def run_case(directory, binary, args, input_bytes=b"", setup=None, guarded=False):
    directory.mkdir(parents=True)
    work, home = directory / "work", directory / "home"
    work.mkdir()
    home.mkdir()
    environment = controlled_environment(home)
    if setup:
        setup(work, environment)
    shutil.copytree(work, directory / "before", symlinks=True)
    shutil.copytree(home, directory / "before-home", symlinks=True)
    (directory / "stdin.bin").write_bytes(input_bytes)
    command = [str(binary), *args]
    if guarded:
        environment.update(PATH=str(home), GIT_EXEC_PATH=str(home))
        command = [*launcher(directory / "guard"), *command]
    manifest = {"binary": str(binary), "argv": args, "guarded": guarded,
                "environment": environment, "cwd": "work", "input": "stdin.bin"}
    write_json(directory / "replay.json", manifest)
    output = subprocess.run(command, input=input_bytes, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, cwd=work, env=environment, timeout=30)
    (directory / "stdout.bin").write_bytes(output.stdout)
    (directory / "stderr.bin").write_bytes(output.stderr)
    result = {"returncode": output.returncode, "stdout": output.stdout.hex(),
              "stderr": output.stderr.hex(), "state": snapshot(work)}
    write_json(directory / "result.json", result)
    return result


def replay(directory, binary=None):
    manifest = json.loads((directory / "replay.json").read_text())
    work = directory / "replay-work"
    shutil.copytree(directory / "before", work, symlinks=True)
    home = directory / "replay-home"
    shutil.copytree(directory / "before-home", home, symlinks=True)
    environment = manifest["environment"]
    for key in ("HOME", "USERPROFILE", "XDG_CONFIG_HOME"):
        environment[key] = str(home)
    environment["GIT_CONFIG_GLOBAL"] = str(home / "empty-config")
    command = [str(binary or manifest["binary"]), *manifest["argv"]]
    if manifest["guarded"]:
        environment.update(PATH=str(home), GIT_EXEC_PATH=str(home))
        command = [*launcher(directory / "replay-guard"), *command]
    return subprocess.run(command, cwd=work, env=environment,
                          input=(directory / manifest["input"]).read_bytes()).returncode


def qualify(reference, candidate, probe, output):
    if output.exists():
        raise RuntimeError("qualification directory must be new; evidence is never overwritten")
    output.mkdir(parents=True)
    checks = []

    def record(name, passed, evidence=None):
        checks.append({"name": name, "passed": bool(passed), "evidence": evidence})
        print(f"{'PASS' if passed else 'FAIL'} {name}", flush=True)

    def initialize(work, environment):
        subprocess.run([str(reference), "init", "-q", "--initial-branch=main", str(work)],
                       env=environment, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def locked(work, environment):
        initialize(work, environment)
        (work / ".git/refs/heads/main.lock").write_bytes(b"another writer\0")

    scenarios = [
        ("binary-input", ["hash-object", "--stdin"], b"\0\xff\r\n", None),
        ("object-write-sha1", ["hash-object", "-w", "--stdin"], b"binary\0blob\xff\n", initialize),
        ("invalid-object", ["cat-file", "-p", "f" * 40], b"", initialize),
        ("preserve-competing-lock", ["update-ref", "refs/heads/main", "f" * 40], b"", locked),
    ]
    # Two repetitions expose fixture/environment drift; equality includes all
    # metadata bytes and lock failure effects, not just stdout.
    for repetition in range(2):
        for name, args, data, setup in scenarios:
            directory = output / f"reference-{repetition}-{name}"
            left = run_case(directory / "left", reference, args, data, setup)
            right = run_case(directory / "right", reference, args, data, setup)
            record(directory.name, equivalent(left, right), str(directory.relative_to(output)))

    negatives = [("exit-status", ["echo"], ["exit"]),
                 ("stderr-routing", ["stdout"], ["stderr"]),
                 ("line-endings", ["lf"], ["crlf"])]
    for name, left_args, right_args in negatives:
        directory = output / f"mismatch-{name}"
        left = run_case(directory / "left", probe, left_args)
        right = run_case(directory / "right", probe, right_args)
        record(name + "-detected", not equivalent(left, right), str(directory.relative_to(output)))
    # A state-only mismatch must fail even with identical process results.
    left = run_case(output / "mismatch-state/left", probe, ["echo"])
    right = run_case(output / "mismatch-state/right", probe, ["echo"],
                     setup=lambda work, env: (work / "unexpected.lock").write_bytes(b"lock"))
    record("state-only-mismatch-detected", not equivalent(left, right), "mismatch-state")
    # Exercise a relocated reproducer with the same recorded input/pre-state.
    fixture = output / "relocated-reproducer"
    shutil.copytree(output / "mismatch-line-endings/right", fixture, symlinks=True)
    result = subprocess.run([sys.executable, str(Path(__file__)), "--replay", str(fixture)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    record("reproducer-replays-exact-bytes", result.returncode == 0 and result.stdout == b"line\r\n")

    data = bytes(range(256)) * 1024
    echo = run_case(output / "guard-echo", probe, ["echo"], data, guarded=True)
    record("guard-preserves-binary-io", echo["returncode"] == 0 and bytes.fromhex(echo["stdout"]) == data)
    status = run_case(output / "guard-exit-status", probe, ["exit"], guarded=True)
    record("guard-preserves-exit-status", status["returncode"] == 7)
    routing = run_case(output / "guard-stderr", probe, ["stderr"], guarded=True)
    record("guard-preserves-stderr-routing", routing["returncode"] == 0
           and routing["stdout"] == "" and bytes.fromhex(routing["stderr"]) == b"same bytes\n")
    for mode in ("fallback", "missing-helper"):
        plain = run_case(output / f"probe-{mode}-control", probe, [mode])
        record(f"{mode}-probe-works-unguarded", plain["returncode"] == 0)
    if os.name != "nt":
        plain = run_case(output / "probe-direct-exec-control", probe, ["direct-exec", str(reference)])
        record("direct-exec-probe-works-unguarded", plain["returncode"] == 0)
    modes = ("fallback", "missing-helper") if os.name == "nt" else ("fallback", "missing-helper", "direct-exec")
    for mode in modes:
        args = [mode, str(reference)] if mode == "direct-exec" else [mode]
        result = run_case(output / f"guard-{mode}", probe, args, guarded=True)
        record(f"guard-blocks-{mode}", result["returncode"] != 0)
    version = run_case(output / "guard-candidate", candidate, ["version"], guarded=True)
    record("guard-launches-candidate", version["returncode"] == 0)

    # Interrupt the actual writer after its durable marker, preserving partial
    # state. This tests the measurement machinery, not Ferragit crash safety.
    for side in ("left", "right"):
        work = output / "interrupted-write" / side
        work.mkdir(parents=True)
        process = subprocess.Popen([*launcher(work.parent / f"{side}-guard"), str(probe), "partial-write"], cwd=work,
                                   env=controlled_environment(work), stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 10
            while not (work / "ready").exists() and time.monotonic() < deadline:
                if process.poll() is not None:
                    break
                time.sleep(.01)
            if not (work / "ready").exists():
                raise RuntimeError("partial writer did not reach marker")
            writer_pid = int((work / "ready").read_text())
            if os.name == "nt":
                # Windows has no POSIX signal/exec contract. Open only this
                # disposable probe process, with PROCESS_TERMINATE access.
                import ctypes
                from ctypes import wintypes
                kernel = ctypes.WinDLL("kernel32", use_last_error=True)
                kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
                kernel.OpenProcess.restype = wintypes.HANDLE
                kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
                kernel.CloseHandle.argtypes = [wintypes.HANDLE]
                handle = kernel.OpenProcess(1, False, writer_pid)
                if not handle:
                    raise ctypes.WinError(ctypes.get_last_error())
                try:
                    if not kernel.TerminateProcess(handle, 73):
                        raise ctypes.WinError(ctypes.get_last_error())
                finally:
                    kernel.CloseHandle(handle)
            else:
                os.kill(writer_pid, signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=10)
            (work / "ready").unlink()  # PID is synchronization, not repository state.
            write_json(work.parent / f"{side}.json", {"returncode": process.returncode,
                       "stdout": stdout.hex(), "stderr": stderr.hex(), "state": snapshot(work)})
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()
    left = json.loads((output / "interrupted-write/left.json").read_text())
    right = json.loads((output / "interrupted-write/right.json").read_text())
    record("interrupted-writes-retain-equal-partial-state", equivalent(left, right)
           and left["returncode"] == (73 if os.name == "nt" else -signal.SIGTERM)
           and len(left["state"]) == 1, "interrupted-write")
    report = {"schema": 1, "platform": platform.platform(), "architecture": platform.machine(),
              "python": sys.version, "reference": str(reference), "candidate": str(candidate),
              "scope": "Phase 0 measurement qualification; not full Git compatibility",
              "reference_sha256": hashlib.sha256(reference.read_bytes()).hexdigest(),
              "reference_build_options": subprocess.check_output([str(reference), "version", "--build-options"]).decode(),
              "checks": checks, "passed": all(check["passed"] for check in checks)}
    write_json(output / "qualification.json", report)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--replay", type=Path)
    args = parser.parse_args()
    if args.replay:
        return replay(args.replay.resolve())
    if not all((args.reference, args.candidate, args.probe, args.output)):
        parser.error("reference, candidate, probe and output are required")
    return qualify(args.reference.resolve(strict=True), args.candidate.resolve(strict=True),
                   args.probe.resolve(strict=True), args.output.resolve())


if __name__ == "__main__":
    raise SystemExit(main())

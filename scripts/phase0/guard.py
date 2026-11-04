# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Test-only candidate launcher. Never used by Ferragit's runtime crates."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid


def windows_process(argv):
    """Assign a suspended process to a one-process, non-breakaway Job Object."""
    from ctypes import wintypes as w

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    size = ctypes.c_size_t
    handle = w.HANDLE

    class Basic(ctypes.Structure):
        _fields_ = [("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64),
                    ("flags", w.DWORD), ("minimum", size), ("maximum", size),
                    ("active", w.DWORD), ("affinity", size), ("priority", w.DWORD),
                    ("scheduling", w.DWORD)]

    class Counters(ctypes.Structure):
        _fields_ = [(key, ctypes.c_uint64) for key in
                    ("read", "write", "other", "read_bytes", "write_bytes", "other_bytes")]

    class Limits(ctypes.Structure):
        _fields_ = [("basic", Basic), ("io", Counters), ("process_memory", size),
                    ("job_memory", size), ("peak_process", size), ("peak_job", size)]

    class Startup(ctypes.Structure):
        _fields_ = [("cb", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
                    ("title", w.LPWSTR), ("x", w.DWORD), ("y", w.DWORD),
                    ("width", w.DWORD), ("height", w.DWORD), ("chars_x", w.DWORD),
                    ("chars_y", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
                    ("show", w.WORD), ("reserved_size", w.WORD),
                    ("reserved_bytes", ctypes.POINTER(ctypes.c_ubyte)),
                    ("stdin", handle), ("stdout", handle), ("stderr", handle)]

    class Process(ctypes.Structure):
        _fields_ = [("process", handle), ("thread", handle),
                    ("pid", w.DWORD), ("tid", w.DWORD)]

    signatures = {
        "CreateJobObjectW": ([ctypes.c_void_p, w.LPCWSTR], handle),
        "SetInformationJobObject": ([handle, ctypes.c_int, ctypes.c_void_p, w.DWORD], w.BOOL),
        "AssignProcessToJobObject": ([handle, handle], w.BOOL),
        "CreateProcessW": ([w.LPCWSTR, w.LPWSTR, ctypes.c_void_p, ctypes.c_void_p,
                            w.BOOL, w.DWORD, ctypes.c_void_p, w.LPCWSTR,
                            ctypes.POINTER(Startup), ctypes.POINTER(Process)], w.BOOL),
        "GetStdHandle": ([w.DWORD], handle),
        "SetHandleInformation": ([handle, w.DWORD, w.DWORD], w.BOOL),
        "ResumeThread": ([handle], w.DWORD),
        "WaitForSingleObject": ([handle, w.DWORD], w.DWORD),
        "GetExitCodeProcess": ([handle, ctypes.POINTER(w.DWORD)], w.BOOL),
        "TerminateProcess": ([handle, w.UINT], w.BOOL),
        "CloseHandle": ([handle], w.BOOL),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(kernel, name)
        function.argtypes, function.restype = arguments, result

    def check(result):
        if not result:
            raise ctypes.WinError(ctypes.get_last_error())
        return result

    job = check(kernel.CreateJobObjectW(None, None))
    process = Process()
    try:
        limits = Limits()
        # ACTIVE_PROCESS + KILL_ON_JOB_CLOSE; no BREAKAWAY_OK flags.
        limits.basic.flags = 0x8 | 0x2000
        limits.basic.active = 1
        check(kernel.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits)))
        startup = Startup()
        startup.cb, startup.flags = ctypes.sizeof(startup), 0x100
        for field, number in (("stdin", -10), ("stdout", -11), ("stderr", -12)):
            value = kernel.GetStdHandle(number & 0xffffffff)
            check(kernel.SetHandleInformation(value, 1, 1))
            setattr(startup, field, value)
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline(argv))
        check(kernel.CreateProcessW(argv[0], command, None, None, True,
                                    0x4, None, None, ctypes.byref(startup), ctypes.byref(process)))
        try:
            check(kernel.AssignProcessToJobObject(job, process.process))
            if kernel.ResumeThread(process.thread) == 0xffffffff:
                raise ctypes.WinError(ctypes.get_last_error())
            check(kernel.WaitForSingleObject(process.process, 0xffffffff) == 0)
            code = w.DWORD()
            check(kernel.GetExitCodeProcess(process.process, ctypes.byref(code)))
            return code.value
        except BaseException:
            kernel.TerminateProcess(process.process, 125)
            raise
    finally:
        for value in (process.thread, process.process, job):
            if value:
                kernel.CloseHandle(value)


def trace_violations(source):
    executions = 0
    violations = []
    for line in source.splitlines():
        if re.search(r"\bexecve(?:at)?\(", line):
            executions += 1
            if executions > 1:
                violations.append(line)
        if re.search(r"\b(?:fork|vfork|clone|clone3)\(", line) and "CLONE_THREAD" not in line:
            violations.append(line)
    if executions == 0:
        violations.append("no initial exec observed")
    return violations


def launcher(evidence):
    """Resolve tools before the candidate's PATH is restricted."""
    command = [sys.executable, str(Path(__file__).resolve()), "--evidence", str(evidence)]
    if sys.platform == "linux":
        tracer = shutil.which("strace")
        if not tracer:
            raise RuntimeError("Linux guard requires strace")
        command += ["--tracer", str(Path(tracer).resolve(strict=True))]
    return [*command, "--"]


def launch(argv, evidence, tracer=None):
    binary = Path(argv[0]).resolve(strict=True)
    argv = [str(binary), *argv[1:]]
    evidence.mkdir(parents=True, exist_ok=True)
    stem = evidence / str(uuid.uuid4())
    with binary.open("rb") as stream:
        binary_hash = hashlib.file_digest(stream, "sha256").hexdigest()
    record = {"schema": 1, "binary": str(binary), "argv": argv[1:],
              "binary_sha256": binary_hash,
              "platform": sys.platform, "violations": []}
    start = time.monotonic()
    if sys.platform == "linux":
        tracer = tracer or shutil.which("strace")
        if not tracer or not Path(tracer).is_absolute():
            raise RuntimeError("Linux guard requires an absolute strace path; unguarded execution is refused")
        trace = stem.with_suffix(".trace")
        code = subprocess.call([tracer, "-f", "-qq", "-e", "trace=process", "-o", str(trace), *argv])
        events = trace.read_text()
        # strace reports a signalled tracee as 128+signal. Preserve the actual
        # signal at the launcher boundary instead of converting it to exit 143.
        initial = re.search(r"^(\d+)\s+execve", events, re.M)
        if initial:
            killed = re.search(r"^" + initial[1] + r"\s+\+\+\+ killed by (SIG\w+)", events, re.M)
            if killed:
                code = -getattr(signal, killed[1])
        record.update(mode="strace-process-events", trace=str(trace),
                      violations=trace_violations(events))
    elif sys.platform == "darwin":
        # sandbox-exec starts the allowed executable in this policy. Subsequent
        # forks and execution of other programs are denied by the kernel.
        policy = '(version 1)(allow default)(deny process-fork)(deny process-exec)'
        policy += '(allow process-exec (literal ' + json.dumps(str(binary)) + '))'
        code = subprocess.call(["/usr/bin/sandbox-exec", "-p", policy, *argv])
        record.update(mode="macos-kernel-process-policy", policy=policy,
                      observation="enforcement; denied attempts are not an event trace")
    elif sys.platform == "win32":
        code = windows_process(argv)
        record.update(mode="windows-one-process-job",
                      observation="enforcement; denied attempts are not an event trace")
    else:
        raise RuntimeError("no process guard for this platform")
    record.update(returncode=code, seconds=time.monotonic() - start)
    stem.with_suffix(".json").write_text(json.dumps(record, indent=2) + "\n")
    return 125 if record["violations"] else code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--tracer", type=Path)
    parser.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    if not argv:
        parser.error("missing candidate executable")
    try:
        code = launch(argv, args.evidence, args.tracer)
    except Exception as error:
        print(f"process guard: {error}", file=sys.stderr)
        code = 125
    if code < 0:
        signal.signal(-code, signal.SIG_DFL)
        os.kill(os.getpid(), -code)
    raise SystemExit(code)


if __name__ == "__main__":
    main()

# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Provision checksum-pinned test references without replacing system Git."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
UPSTREAM = {
    "version": "2.56.0",
    "url": "https://www.kernel.org/pub/software/scm/git/git-2.56.0.tar.xz",
    "sha256": "26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3",
    "signature_url": "https://www.kernel.org/pub/software/scm/git/git-2.56.0.tar.sign",
    "signature_sha256": "bee7d2f96844063557b07b43811d81e1fc54563e0da2c36a975dfd432b196a5c",
}
WINDOWS = {
    "version": "2.56.0.windows.4", "repository": "git-for-windows/git",
    "resource": "releases/assets/677312658",
    "name": "PortableGit-2.56.0.4-64-bit.7z.exe",
    "sha256": "b1f3e52216e2ed6882d1bebfd4d3fdf319bec40d2edbbf9fd26ec4fd3fce7ed1",
}
FLAGS = ["NO_GETTEXT=YesPlease", "NO_TCLTK=YesPlease", "NO_PERL=YesPlease",
         "NO_PYTHON=YesPlease", "NO_INSTALL_HARDLINKS=YesPlease"]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checked_download(url, destination, expected):
    if not destination.exists():
        temporary = destination.with_suffix(destination.suffix + ".download")
        with urllib.request.urlopen(url, timeout=120) as source, temporary.open("xb") as output:
            shutil.copyfileobj(source, output)
        if digest(temporary) != expected:
            temporary.unlink()
            raise RuntimeError("reference artifact checksum mismatch")
        temporary.rename(destination)
    if digest(destination) != expected:
        raise RuntimeError("cached reference artifact checksum mismatch")


def file_manifest(root):
    files = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            files.append({"path": path.relative_to(root).as_posix(), "symlink": os.readlink(path)})
        elif path.is_file():
            files.append({"path": path.relative_to(root).as_posix(), "bytes": path.stat().st_size,
                          "sha256": digest(path)})
    return files


def provision(destination, ferragit):
    destination.mkdir(parents=True, exist_ok=True)
    prefix = destination / "install"
    if prefix.exists():
        raise RuntimeError("reference installation already exists; use a new output directory")
    log = destination / "build.log"
    if sys.platform == "win32":
        if platform.machine().lower() not in ("amd64", "x86_64"):
            raise RuntimeError("this pinned Windows profile requires x86_64")
        artifact = destination / WINDOWS["name"]
        if not artifact.exists():
            subprocess.run([str(ferragit), "repo", "download", "--anonymous", WINDOWS["repository"],
                            WINDOWS["resource"], str(artifact)], check=True)
        if digest(artifact) != WINDOWS["sha256"]:
            raise RuntimeError("Windows reference checksum mismatch")
        extractor = shutil.which("7z")
        if not extractor:
            raise RuntimeError("Windows reference provisioning requires 7-Zip")
        home = destination / "install-home"
        home.mkdir()
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("GIT_", "FERRAGIT_"))}
        environment.update(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(home),
                           GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=str(home / "empty-config"))
        with log.open("wb") as output:
            subprocess.run([extractor, "x", str(artifact), "-y", f"-o{prefix}"], stdout=output,
                           stderr=subprocess.STDOUT, check=True, timeout=300)
            # The pinned batch ends with DEL post-install.bat. Execute an
            # identical adjacent copy so cmd can finish reading its input.
            # Its %~dp0 still names the reference installation directory.
            post_install = prefix / "post-install.bat"
            post_install_hash = digest(post_install)
            runner = prefix / "ferragit-post-install.cmd"
            with runner.open("xb") as stream:
                stream.write(post_install.read_bytes())
            try:
                subprocess.run([os.environ["COMSPEC"], "/d", "/c", runner.name], cwd=prefix,
                               env=environment, stdout=output, stderr=subprocess.STDOUT,
                               check=True, timeout=120)
            finally:
                runner.unlink()
        binary = prefix / "cmd/git.exe"
        source = None
        recipe = {"artifact": WINDOWS, "installation": "7-Zip extraction and upstream post-install; isolated home",
                  "extractor": extractor, "extractor_sha256": digest(Path(extractor)),
                  "post_install_sha256": post_install_hash,
                  "post_install_invocation": "identical adjacent copy; original self-delete preserved"}
    elif sys.platform in ("darwin", "linux"):
        artifact = destination / "git-2.56.0.tar.xz"
        checked_download(UPSTREAM["url"], artifact, UPSTREAM["sha256"])
        signature = destination / "git-2.56.0.tar.sign"
        checked_download(UPSTREAM["signature_url"], signature, UPSTREAM["signature_sha256"])
        # Upstream has its own Cargo package; keep it outside the host workspace.
        build_root = Path(tempfile.mkdtemp(prefix="ferragit-reference-"))
        source = build_root / "git-2.56.0"
        with tarfile.open(artifact) as archive:
            archive.extractall(build_root, filter="data")
        command = ["make", "-j4", f"prefix={prefix}", *FLAGS]
        try:
            with log.open("wb") as output:
                for target in ("all", "install"):
                    subprocess.run([*command, target], cwd=source, stdout=output,
                                   stderr=subprocess.STDOUT, check=True)
        except subprocess.CalledProcessError:
            print(log.read_text(errors="replace")[-16000:], file=sys.stderr)
            raise
        binary = prefix / "bin/git"
        recipe = {"artifact": UPSTREAM, "argv": command,
                  "profile": "core harness reference; optional gettext/Tcl/Perl/Python disabled",
                  "compiler": subprocess.check_output(["cc", "--version"]).decode(),
                  "make": subprocess.check_output(["make", "--version"]).decode(),
                  "rustc": subprocess.check_output(["rustc", "--version"]).decode() if shutil.which("rustc") else None}
    else:
        raise RuntimeError("unsupported reference platform")
    options = subprocess.check_output([str(binary), "version", "--build-options"]).decode()
    expected = WINDOWS["version"] if sys.platform == "win32" else UPSTREAM["version"]
    if options.splitlines()[0] != f"git version {expected}":
        raise RuntimeError("installed reference version mismatch")
    manifest = {"schema": 1, "platform": platform.platform(), "architecture": platform.machine(),
                "binary": str(binary), "binary_sha256": digest(binary),
                "build_options": options, "recipe": recipe,
                "source_build_directory": str(source) if source else None,
                "source_build_directory_retained": False, "files": file_manifest(prefix),
                "signature_verification": "artifact checksum pinned; see compat/baselines.json for tag verification"}
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if source is not None:
        shutil.rmtree(source.parent)
    print(json.dumps({"reference": str(binary), "sha256": manifest["binary_sha256"]}))
    return binary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ferragit", type=Path, required=True)
    args = parser.parse_args()
    provision(args.output.resolve(), args.ferragit.resolve(strict=True))

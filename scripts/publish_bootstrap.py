# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Launch ferragit for provisioning, push and verification; never invoke C Git.

ferragit reads macOS Keychain directly or FERRAGIT_TOKEN on other platforms. This
wrapper does not retrieve, inspect, store, or forward credentials.
"""
import argparse
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(binary, *args):
    return subprocess.check_output([str(binary), *args], cwd=ROOT).decode().strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--owner", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--candidate", default="target/debug/ferragit")
    parser.add_argument("--publish", action="store_true", help="create the private repo and push")
    args = parser.parse_args()
    full_name = f"{args.owner}/{args.name}"
    binary = (ROOT / args.candidate).resolve()
    files = [name for name in run(binary, "ls-files", "-z").split("\0") if name]
    if not files:
        raise RuntimeError("the Rust-created index is empty")
    for name in files:
        if name.startswith((".cache/", "target/", "artifacts/", ".git/")) or "__pycache__" in pathlib.PurePosixPath(name).parts:
            raise RuntimeError(f"refusing generated/private content: {name}")
    head = run(binary, "rev-parse", "HEAD")
    summary = {"repository": full_name, "initial_creation_visibility": "private", "commit": head, "tracked_files": len(files)}
    if not args.publish:
        print(json.dumps(dict(summary, action="dry-run; no repository created")))
        return
    output = ROOT / "artifacts"
    output.mkdir(exist_ok=True)
    state_path = output / "created-repository.json"
    if state_path.exists():
        state = json.loads(state_path.read_text())
        if state["full_name"].lower() != full_name.lower():
            raise RuntimeError("creation receipt belongs to a different repository")
        remote = json.loads(run(binary, "repo", "inspect", full_name))
        if remote["id"] != state["id"]:
            raise RuntimeError("recorded repository identity changed")
    else:
        # Collisions fail in ferragit; an unrelated existing repository is never reused.
        print(run(binary, "repo", "create", "--private", full_name), flush=True)
        state = json.loads(run(binary, "repo", "inspect", full_name))
        state_path.write_text(json.dumps(state, indent=2) + "\n")
    subprocess.run([str(binary), "push", state["clone_url"], "HEAD:refs/heads/main"], cwd=ROOT, check=True)
    receipt = json.loads(run(binary, "repo", "verify", full_name))
    if receipt["commit"] != head or receipt["verified_file_count"] != len(files):
        raise RuntimeError("verification receipt differs from the reviewed local snapshot")
    (output / "publish-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(dict(receipt, action="verified")))


if __name__ == "__main__":
    main()


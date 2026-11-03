# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Capture immutable reference identifiers and a command inventory (no Git code imported)."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
HEADERS = {"User-Agent": "ferragit-baseline-capture", "Accept": "application/vnd.github+json"}


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=30) as response:
        return response.read()


def capture(repository, tag):
    base = f"https://api.github.com/repos/{repository}/git"
    reference = json.loads(get(f"{base}/ref/tags/{tag}"))
    obj = reference["object"]
    tag_object = None
    verification = None
    while obj["type"] == "tag":
        annotated = json.loads(get(obj["url"]))
        tag_object = obj["sha"]
        verification = annotated.get("verification")
        obj = annotated["object"]
    if obj["type"] != "commit":
        raise RuntimeError(f"{repository} {tag} does not resolve to a commit")
    return {
        "repository": repository,
        "tag": tag,
        "commit": obj["sha"],
        "tag_object": tag_object,
        "github_signature_verification": verification,
        "local_gpg_verification": "not performed",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--upstream-tag", default="v2.56.0")
    parser.add_argument("--windows-tag", default="v2.56.0.windows.4")
    args = parser.parse_args()
    sources = [
        capture("git/git", args.upstream_tag),
        capture("git-for-windows/git", args.windows_tag),
    ]
    command_url = f"https://raw.githubusercontent.com/git/git/{sources[0]['commit']}/command-list.txt"
    raw = get(command_url)
    commands = []
    for line in raw.decode().splitlines():
        fields = line.split()
        if fields and fields[0].startswith("git-"):
            commands.append({
                "name": fields[0][4:],
                "source_categories": fields[1:],
                "status": "unimplemented",
                "option_inventory": "pending source/parser audit",
            })
    known = {"init", "hash-object", "cat-file", "add", "write-tree", "commit", "rev-parse", "ls-files", "push", "version"}
    for command in commands:
        if command["name"] in known:
            command["status"] = "partial"
    git = pathlib.Path(shutil.which("git")).resolve()
    local = {
        "path": str(git),
        "sha256": hashlib.sha256(git.read_bytes()).hexdigest(),
        "build_options": subprocess.check_output([str(git), "version", "--build-options"], text=True),
        "role": "available local oracle; not the pinned target release",
    }
    output = ROOT / "compat"
    output.mkdir(exist_ok=True)
    (output / "baselines.json").write_text(json.dumps({
        "SPDX-FileCopyrightText": "2026 Guido <guido@ngjansen.be>",
        "SPDX-License-Identifier": "GPL-2.0-only",
        "schema": 1,
        "targets": sources,
        "local_oracle": local,
        "qualification": "not qualified against target releases",
        "reference_build_profiles": "pending pinned builds on each advertised platform",
    }, indent=2) + "\n")
    (output / "commands.json").write_text(json.dumps({
        "SPDX-FileCopyrightText": "2026 Guido <guido@ngjansen.be>",
        "SPDX-License-Identifier": "GPL-2.0-only",
        "schema": 1,
        "source": command_url,
        "source_sha256": hashlib.sha256(raw).hexdigest(),
        "scope": "command-list entries only; installed helpers, build-optional commands, and exhaustive options still require audit",
        "commands": commands,
    }, indent=2) + "\n")
    print(json.dumps({"targets": [{"tag": x["tag"], "commit": x["commit"]} for x in sources], "command_entries": len(commands)}))


if __name__ == "__main__":
    main()


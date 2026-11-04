# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Capture immutable reference identifiers and a command inventory (no Git code imported)."""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
FERRAGIT = ROOT / "target/debug" / ("ferragit.exe" if os.name == "nt" else "ferragit")


def get(repository, resource):
    return json.loads(subprocess.check_output([str(FERRAGIT), "repo", "api", repository, resource]))


def capture(repository, tag):
    reference = get(repository, f"git/ref/tags/{tag}")
    obj = reference["object"]
    tag_object = None
    verification = None
    while obj["type"] == "tag":
        annotated = get(repository, f"git/tags/{obj['sha']}")
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
    content = get("git/git", f"contents/command-list.txt?ref={sources[0]['commit']}")
    if content.get("encoding") != "base64":
        raise RuntimeError("unsupported command-list encoding")
    raw = base64.b64decode(content["content"])
    old_commands = json.loads((ROOT / "compat/commands.json").read_text())
    previous = {item["name"]: item for item in old_commands["commands"]}
    if old_commands["source_sha256"] != hashlib.sha256(raw).hexdigest():
        raise RuntimeError("command list changed; inventory requires explicit baseline review")
    commands = []
    for line in raw.decode().splitlines():
        fields = line.split()
        if fields and fields[0].startswith("git-"):
            commands.append(previous.get(fields[0][4:], {
                "name": fields[0][4:],
                "source_categories": fields[1:],
                "status": "unimplemented",
                "option_inventory": "pending source/parser audit",
            }))
    upstream_names = {entry["name"] for entry in commands}
    commands += [entry for name, entry in previous.items() if name not in upstream_names]
    commands.sort(key=lambda entry: entry["name"])
    git = pathlib.Path(shutil.which("git")).resolve()
    local = {
        "path": str(git),
        "sha256": hashlib.sha256(git.read_bytes()).hexdigest(),
        "build_options": subprocess.check_output([str(git), "version", "--build-options"], text=True),
        "role": "available local oracle; not the pinned target release",
    }
    output = ROOT / "compat"
    output.mkdir(exist_ok=True)
    existing = json.loads((output / "baselines.json").read_text())
    if [(s["repository"], s["commit"]) for s in sources] != [(s["repository"], s["commit"]) for s in existing["targets"]]:
        raise RuntimeError("reference commits changed; explicit qualification review is required")
    for source, old in zip(sources, existing["targets"]):
        source["local_gpg_verification"] = old["local_gpg_verification"]
    (output / "baselines.json").write_text(json.dumps({
        **existing,
        "SPDX-FileCopyrightText": "2026 Guido <guido@ngjansen.be>",
        "SPDX-License-Identifier": "GPL-2.0-only",
        "schema": 1,
        "targets": sources,
        "local_oracle": local,
    }, indent=2) + "\n")
    (output / "commands.json").write_text(json.dumps({
        **old_commands,
        "SPDX-FileCopyrightText": "2026 Guido <guido@ngjansen.be>",
        "SPDX-License-Identifier": "GPL-2.0-only",
        "schema": 1,
        "source": command_url,
        "source_sha256": hashlib.sha256(raw).hexdigest(),
        "scope": old_commands["scope"],
        "commands": commands,
    }, indent=2) + "\n")
    print(json.dumps({"targets": [{"tag": x["tag"], "commit": x["commit"]} for x in sources], "command_entries": len(commands)}))


if __name__ == "__main__":
    main()


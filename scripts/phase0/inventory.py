# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Index interface declarations in pinned sources; retain audit gaps explicitly."""
import argparse
import hashlib
import json
from pathlib import Path
import re

ATTRIBUTION = {"SPDX-FileCopyrightText": "2026 Guido <guido@ngjansen.be>",
               "SPDX-License-Identifier": "GPL-2.0-only"}
SUFFIXES = {".c", ".h", ".sh", ".perl", ".pm", ".py", ".tcl", ".adoc", ".txt"}
PATTERNS = {
    "documented-option": re.compile(r"(?<![\w-])--[a-zA-Z][a-zA-Z0-9-]*"),
    "option-macro": re.compile(r"\bOPT_[A-Z_]+\s*\([^,\n]*,\s*\"([^\"]+)\""),
    "environment-name": re.compile(r"\bGIT_[A-Z][A-Z0-9_]+\b"),
    "configuration-definition": re.compile(r"^([a-zA-Z][a-zA-Z0-9_.<>-]+)::", re.M),
    "environment-lookup": re.compile(r"\b(?:getenv|git_env_bool|git_env_ulong|git_env_size_t)\s*\(\s*\"([^\"]+)\""),
}


def inventory(source, profile):
    entries, files, unresolved = {}, [], []
    for path in sorted(source.rglob("*")):
        relative = path.relative_to(source).as_posix()
        if not path.is_file() or path.is_symlink() or path.suffix not in SUFFIXES:
            continue
        if any(part in (".git", "po") for part in path.relative_to(source).parts):
            continue
        raw = path.read_bytes()
        text = raw.decode("utf-8", errors="replace")
        files.append({"path": relative, "sha256": hashlib.sha256(raw).hexdigest()})
        for category, pattern in PATTERNS.items():
            if category == "configuration-definition" and not relative.startswith("Documentation/config/"):
                continue
            if category == "documented-option" and path.suffix != ".adoc":
                continue
            for match in pattern.finditer(text):
                name = match.group(1) if match.lastindex else match.group()
                key = (category, name)
                entry = entries.setdefault(key, {"kind": category, "name": name,
                    "status": "unimplemented", "audit": "declaration candidate; semantic audit pending",
                    "locations": []})
                entry["locations"].append({"path": relative, "line": text.count("\n", 0, match.start()) + 1})
        # Keep whole parser sources in the denominator, including hand-written
        # argv parsing, computed names, shared macros, subcommands and aliases.
        if path.suffix in {".c", ".sh", ".perl", ".pm", ".py", ".tcl"}:
            kinds = []
            if re.search(r"\b(?:parse_options|parse-options|OPT_|argv|GetOptions|getopt)", text):
                kinds.append("command-options-and-subcommands")
            if re.search(r"\b(?:getenv|git_env_|\$ENV|os\.environ|env\()", text):
                kinds.append("environment-lookups")
            if re.search(r"\b(?:git_config|git_configset|config_get|starts_with\(var)", text):
                kinds.append("configuration-and-dynamic-families")
            if kinds and not relative.startswith(("t/", "Documentation/")):
                unresolved.append({"path": relative, "surfaces": kinds,
                                   "status": "source/parser audit pending"})
    commands = []
    for line in (source / "command-list.txt").read_text().splitlines():
        fields = line.split()
        if fields and fields[0].startswith("git-"):
            commands.append({"name": fields[0][4:], "categories": fields[1:], "status": "unimplemented"})
    return {**ATTRIBUTION, "schema": 1, "profile": profile,
            "scope": "source discovery catalog; not a complete semantic interface inventory",
            "commands": commands, "declarations": list(entries.values()),
            "unresolved_parser_sources": unresolved, "source_files": files,
            "gaps": ["hand-written/dynamic option and config semantics require source audit",
                     "installed helpers and distribution components require installed-file audit",
                     "build/platform conditionals and shared parser families require attribution",
                     "implementation/test ownership must be assigned before verification"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = inventory(args.source.resolve(strict=True), args.profile)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"commands": len(result["commands"]), "declaration_candidates": len(result["declarations"]),
                      "unresolved_parser_sources": len(result["unresolved_parser_sources"])}))

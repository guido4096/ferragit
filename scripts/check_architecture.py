# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Enforce the milestone's no-child-process/no-other-Git runtime boundary."""
import pathlib
import re
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
errors = []
for crate in ("git-cli", "git-core", "git-platform"):
    for path in (ROOT / "crates" / crate / "src").rglob("*.rs"):
        source = path.read_text()
        if re.search(r"\b(Command|Stdio|Child)\b|extern\s+\"|libgit2|\bgix::", source):
            errors.append(f"{path.relative_to(ROOT)}: forbidden runtime process/FFI/Git-engine reference")
lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
for package in lock["package"]:
    if package["name"] in ("git2", "libgit2-sys", "gix", "gitoxide"):
        errors.append(f"forbidden Git engine dependency: {package['name']}")
if errors:
    raise SystemExit("\n".join(errors))
print("Runtime architecture boundary passed (source/dependency check; not an OS sandbox).")


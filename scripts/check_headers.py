# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Check original project headers and generated-data attribution."""
import json
import pathlib
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
HOLDER = "2026 Guido <guido@ngjansen.be>"
LICENSE = "GPL-2.0-only"
paths = list((ROOT / "crates").rglob("*.rs")) + list((ROOT / "scripts").rglob("*.py"))
paths += list((ROOT / "crates").glob("*/Cargo.toml"))
paths += list((ROOT / ".github").rglob("*.yml"))
paths += [ROOT / p for p in ("Cargo.toml", "REUSE.toml", ".gitignore", "README.md", "PLAN.md", "THIRD_PARTY.md", "AGENTS.md")]
paths += list((ROOT / "docs").glob("*.md"))
errors = []
for path in paths:
    header = path.read_text()[:250]
    if f"SPDX-FileCopyrightText: {HOLDER}" not in header or f"SPDX-License-Identifier: {LICENSE}" not in header:
        errors.append(str(path.relative_to(ROOT)))
for path in (ROOT / "compat").glob("*.json"):
    data = json.loads(path.read_text())
    if data.get("SPDX-FileCopyrightText") != HOLDER or data.get("SPDX-License-Identifier") != LICENSE:
        errors.append(str(path.relative_to(ROOT)))
manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
assert manifest["workspace"]["package"]["license"] == LICENSE
assert b"GNU GENERAL PUBLIC LICENSE" in (ROOT / "LICENSE").read_bytes()
if errors:
    raise SystemExit("Missing license attribution: " + ", ".join(errors))
print(f"License/copyright checks passed for {len(paths)} source/config/documentation files and compatibility JSON.")


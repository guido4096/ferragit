# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Run pinned-reference and guarded candidate qualification on a native host."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from provision import provision
from guard import launcher


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("artifacts/phase0"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    suffix = ".exe" if sys.platform == "win32" else ""
    candidate = root / f"target/debug/ferragit{suffix}"
    conformance = root / f"target/debug/git-conformance{suffix}"
    probe = root / f"target/debug/process-probe{suffix}"
    output = args.output.resolve()
    reference = provision(output / "reference", candidate)
    subprocess.run([sys.executable, str(Path(__file__).with_name("qualify.py")),
                    "--reference", str(reference), "--candidate", str(candidate),
                    "--probe", str(probe), "--output", str(output / "measurement")], check=True)
    environment = os.environ.copy()
    environment["FERRAGIT_PROCESS_GUARD"] = json.dumps(launcher(output / "candidate-processes"))
    subprocess.run([str(conformance), "--reference", str(reference), "--candidate", str(candidate),
                    "--report", str(output / "conformance.json")], env=environment, check=True)


if __name__ == "__main__":
    main()

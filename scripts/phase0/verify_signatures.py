# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Verify pinned release metadata with an isolated OpenPGP keyring."""
import argparse
import hashlib
import json
import lzma
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def verify_signature(gpg, home, signature, payload, expected):
    result = subprocess.run([gpg, "--homedir", str(home), "--batch", "--no-auto-key-retrieve",
                             "--status-fd", "1", "--verify", str(signature), str(payload)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    valid = [line.split() for line in result.stdout.decode().splitlines()
             if line.startswith("[GNUPG:] VALIDSIG ")]
    if result.returncode != 0 or len(valid) != 1 or valid[0][2] != expected:
        raise RuntimeError(f"OpenPGP verification failed for {payload.name}: "
                           + result.stderr.decode(errors="replace"))
    return {"signing_fingerprint": valid[0][2], "signature_date": valid[0][3],
            "status": "cryptographically-valid", "identity_authority": "pinned fingerprint provenance"}


def verify(output, archive=None, archive_signature=None):
    gpg = shutil.which("gpg")
    if not gpg:
        raise RuntimeError("independent signature verification requires GnuPG")
    if output.exists():
        raise RuntimeError("signature evidence already exists")
    baselines = json.loads((ROOT / "compat/baselines.json").read_text())
    keys = json.loads((ROOT / "compat/signing-keys.json").read_text())["keys"]
    records = []
    with tempfile.TemporaryDirectory(prefix="ferragit-openpgp-") as temporary:
        home = Path(temporary)
        for key in keys:
            raw = key["armored_public_key"].encode()
            if hashlib.sha256(raw).hexdigest() != key["sha256"]:
                raise RuntimeError("pinned public-key checksum mismatch")
            path = home / (key["primary_fingerprint"] + ".asc")
            path.write_bytes(raw)
            subprocess.run([gpg, "--homedir", str(home), "--batch", "--import", str(path)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60)
        listing = subprocess.check_output([gpg, "--homedir", str(home), "--batch",
                                           "--with-colons", "--fingerprint", "--list-keys"])
        fingerprints = {line.split(":")[9] for line in listing.decode().splitlines() if line.startswith("fpr:")}
        if any(key["primary_fingerprint"] not in fingerprints for key in keys):
            raise RuntimeError("imported key does not match its pinned primary fingerprint")
        for target in baselines["targets"]:
            key = next(key for key in keys if key["repository"] == target["repository"])
            signed = target["github_signature_verification"]
            body = signed["payload"].encode()
            signature_bytes = signed["signature"].encode()
            raw_tag = body + signature_bytes
            identity = hashlib.sha1(b"tag " + str(len(raw_tag)).encode() + b"\0" + raw_tag).hexdigest()
            if identity != target["tag_object"] or not body.startswith(
                    f"object {target['commit']}\ntype commit\ntag {target['tag']}\n".encode()):
                raise RuntimeError("signed tag bytes do not match the pinned tag and commit")
            payload, signature = home / "payload", home / "signature.asc"
            payload.write_bytes(body)
            signature.write_bytes(signature_bytes)
            result = verify_signature(gpg, home, signature, payload, key["signing_fingerprint"])
            # The verifier itself must reject a corrupted signed payload.
            payload.write_bytes(body + b"modified payload\n")
            try:
                verify_signature(gpg, home, signature, payload, key["signing_fingerprint"])
            except RuntimeError:
                result["altered_payload_rejected"] = True
            else:
                raise RuntimeError("signature verifier accepted a modified payload")
            records.append({"repository": target["repository"], "tag": target["tag"],
                            "tag_object": identity, "commit": target["commit"], **result})
        if archive is not None:
            from provision import UPSTREAM, digest
            if archive_signature is None or digest(archive) != UPSTREAM["sha256"] or digest(archive_signature) != UPSTREAM["signature_sha256"]:
                raise RuntimeError("release archive/signature checksum mismatch")
            tar = home / "release.tar"
            total = 0
            with lzma.open(archive) as source, tar.open("xb") as destination:
                while chunk := source.read(65536):
                    total += len(chunk)
                    if total > 512 * 1024 * 1024:
                        raise RuntimeError("release tar exceeds verification limit")
                    destination.write(chunk)
            key = next(key for key in keys if key["repository"] == "git/git")
            result = verify_signature(gpg, home, archive_signature, tar, key["signing_fingerprint"])
            records.append({"archive_sha256": digest(archive), "signed_tar_bytes": total, **result})
    report = {"schema": 1, "verifier": subprocess.check_output([gpg, "--version"]).decode(),
              "scope": "independent cryptographic verification; identity trust is the pinned fingerprint provenance",
              "records": records, "passed": True}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Verified {len(records)} signatures with isolated GnuPG; altered payloads rejected", flush=True)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--archive-signature", type=Path)
    args = parser.parse_args()
    verify(args.output, args.archive, args.archive_signature)

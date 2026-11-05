<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Provenance and third-party components

The original Rust implementation and project scripts are authored for this project and attributed to Guido <guido@ngjansen.be>. Git's implementation code and shell test suite have not been copied into the source tree.

Git format/protocol documentation and C Git executable behavior are used as compatibility references. `compat/commands.json` contains command names and categories extracted from the pinned upstream and Git for Windows command lists, with its URL and content checksum. `compat/baselines.json` records upstream tag/commit identifiers and available signature-verification metadata. The Phase 0 tools download reference sources/binaries only to ignored artifact directories and index interface names and source locations. Downloaded reference distributions retain all upstream notices; they are not part of the original Rust source. Public release-signing keys retain their issuer identity; attribution of the metadata compilation does not assert authorship of those keys. These facts do not transfer copyright in upstream source to this project.

Direct third-party runtime libraries are `flate2` (Rust compression backend), `sha1-checked`, `sha2`, `ignore`, `reqwest` (rustls HTTPS backend), `serde_json`, and macOS-only `security-framework` for native Keychain access. The test tooling additionally uses `serde`. Their transitive dependencies are pinned in Cargo.lock. They retain their original authors, copyright notices and licenses; where libraries offer MIT or Apache-2.0, the MIT option can be selected for this GPL-2.0-only project.

Before distributing compiled packages, collect and ship the applicable notices and license texts for the actual target dependency graph, including native cryptographic components. No vendored dependency source has been relicensed or assigned to Guido.

The GPL version 2 text in LICENSE is reproduced from https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt and retains the Free Software Foundation's copyright in the license document itself. The project's choice is GPL-2.0-only, as expressed by Cargo metadata and SPDX headers, even though the license's generic application appendix mentions later versions as an example.

// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! A small smart-HTTP sender. The client constructs packet lines and a complete
//! non-delta pack itself; no Git executable, library, or remote helper is invoked.
use crate::{
    object::{HashKind, ObjectId, ObjectKind},
    repository::{valid_ref, Repository},
    store::read_bounded,
    Error, Result,
};
use flate2::{write::ZlibEncoder, Compression};
use reqwest::{blocking::Client, redirect::Policy, Url};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    time::Duration,
};

pub const MAX_PACK_BYTES: usize = 128 * 1024 * 1024;
const MAX_OBJECTS: usize = 100_000;

pub fn packet(payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > 65516 {
        return Err(Error::Invalid("packet line too large".into()));
    }
    let mut bytes = format!("{:04x}", payload.len() + 4).into_bytes();
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

pub fn read_packets(bytes: &[u8]) -> Result<Vec<Option<&[u8]>>> {
    let mut offset = 0;
    let mut packets = Vec::new();
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(Error::Invalid("truncated packet length".into()));
        }
        let header = std::str::from_utf8(&bytes[offset..offset + 4])
            .map_err(|_| Error::Invalid("non-ASCII packet length".into()))?;
        if !header.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("invalid packet length".into()));
        }
        let length = usize::from_str_radix(header, 16)
            .map_err(|_| Error::Invalid("invalid packet length".into()))?;
        offset += 4;
        if length == 0 {
            packets.push(None);
            continue;
        }
        if !(4..=65520).contains(&length) || length - 4 > bytes.len() - offset {
            return Err(Error::Invalid("truncated or unsupported packet".into()));
        }
        packets.push(Some(&bytes[offset..offset + length - 4]));
        offset += length - 4;
    }
    Ok(packets)
}

#[derive(Debug)]
pub struct Advertisement {
    pub refs: BTreeMap<String, ObjectId>,
    pub capabilities: BTreeSet<String>,
}

impl Advertisement {
    pub fn parse(bytes: &[u8], hash: HashKind) -> Result<Self> {
        let packets = read_packets(bytes)?;
        if packets.len() < 3
            || packets[0] != Some(b"# service=git-receive-pack\n")
            || packets[1].is_some()
            || packets.last() != Some(&None)
        {
            return Err(Error::Invalid("invalid receive-pack advertisement".into()));
        }
        let mut refs = BTreeMap::new();
        let mut capabilities = BTreeSet::new();
        let mut first = true;
        for packet in &packets[2..packets.len() - 1] {
            let bytes =
                packet.ok_or_else(|| Error::Invalid("unexpected advertisement flush".into()))?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| Error::Unsupported("non-UTF-8 remote reference".into()))?
                .trim_end_matches('\n');
            let (reference, caps) = text.split_once('\0').unwrap_or((text, ""));
            if first {
                capabilities.extend(caps.split_whitespace().map(str::to_owned));
                first = false;
            } else if !caps.is_empty() {
                return Err(Error::Invalid("capabilities after first reference".into()));
            }
            let (id, name) = reference
                .split_once(' ')
                .ok_or_else(|| Error::Invalid("bad advertised reference".into()))?;
            let id = ObjectId::parse(hash, id)?;
            if name == "capabilities^{}" && id.bytes().iter().all(|b| *b == 0) {
                continue;
            }
            if !valid_ref(name) {
                return Err(Error::Unsupported("advertised reference syntax".into()));
            }
            if refs.insert(name.to_owned(), id).is_some() {
                return Err(Error::Invalid("duplicate advertised reference".into()));
            }
        }
        if !capabilities.contains("report-status") {
            return Err(Error::Unsupported("server without report-status".into()));
        }
        if capabilities.iter().any(|cap| {
            cap.starts_with("object-format=") && cap != &format!("object-format={}", hash.name())
        }) {
            return Err(Error::Unsupported("remote object hash algorithm".into()));
        }
        Ok(Self { refs, capabilities })
    }
}

fn commit_links(data: &[u8], hash: HashKind) -> Result<Vec<ObjectId>> {
    let mut links = Vec::new();
    let mut saw_tree = false;
    for line in data
        .split(|b| *b == b'\n')
        .take_while(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix(b"tree ") {
            if saw_tree {
                return Err(Error::Invalid("multiple commit trees".into()));
            }
            saw_tree = true;
            links.push(ObjectId::parse(
                hash,
                std::str::from_utf8(value).map_err(|_| Error::Invalid("invalid tree ID".into()))?,
            )?);
        } else if let Some(value) = line.strip_prefix(b"parent ") {
            links.push(ObjectId::parse(
                hash,
                std::str::from_utf8(value)
                    .map_err(|_| Error::Invalid("invalid parent ID".into()))?,
            )?);
        }
    }
    if !saw_tree {
        return Err(Error::Invalid("commit without tree".into()));
    }
    Ok(links)
}

pub fn tree_links(data: &[u8], hash: HashKind) -> Result<Vec<ObjectId>> {
    let mut bytes = data;
    let mut links = Vec::new();
    let width = hash.hex_len() / 2;
    while !bytes.is_empty() {
        let end = bytes
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| Error::Invalid("invalid tree entry".into()))?;
        let header = &bytes[..end];
        let space = header
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| Error::Invalid("invalid tree mode".into()))?;
        if !matches!(
            &header[..space],
            b"40000" | b"100644" | b"100755" | b"120000"
        ) {
            return Err(Error::Unsupported("tree entry mode".into()));
        }
        let name = &header[space + 1..];
        if name.contains(&b'/') {
            return Err(Error::Invalid("slash in tree name".into()));
        }
        crate::index::validate_path(name)?;
        if bytes.len() - end - 1 < width {
            return Err(Error::Invalid("truncated tree object ID".into()));
        }
        let hex = bytes[end + 1..end + 1 + width]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        links.push(ObjectId::parse(hash, &hex)?);
        bytes = &bytes[end + 1 + width..];
    }
    Ok(links)
}

/// Encode all reachable objects, without deltas. This costs bandwidth but is a
/// valid pack and keeps the first push implementation small and independently testable.
pub fn pack(repo: &Repository, head: &ObjectId, ancestor: Option<&ObjectId>) -> Result<Vec<u8>> {
    let mut visited = BTreeSet::new();
    let mut queue = vec![head.clone()];
    let mut objects = Vec::new();
    let mut commit_ids = BTreeSet::new();
    let mut total = 0usize;
    while let Some(id) = queue.pop() {
        if !visited.insert(id.to_string()) {
            continue;
        }
        if visited.len() > MAX_OBJECTS {
            return Err(Error::Unsupported("pack object limit".into()));
        }
        let object = repo.objects.read(&id)?;
        total = total
            .checked_add(object.data.len())
            .ok_or_else(|| Error::Unsupported("pack size overflow".into()))?;
        if total > MAX_PACK_BYTES {
            return Err(Error::Unsupported(
                "pack exceeds 128 MiB uncompressed".into(),
            ));
        }
        match object.kind {
            ObjectKind::Commit => {
                commit_ids.insert(id.to_string());
                queue.extend(commit_links(&object.data, repo.objects.hash)?);
            }
            ObjectKind::Tree => queue.extend(tree_links(&object.data, repo.objects.hash)?),
            ObjectKind::Blob => {}
            ObjectKind::Tag => return Err(Error::Unsupported("pushing annotated tags".into())),
        }
        objects.push(object);
    }
    if ancestor.is_some_and(|id| !commit_ids.contains(&id.to_string())) {
        return Err(Error::Invalid(
            "push rejected: non-fast-forward (fetch is not implemented yet)".into(),
        ));
    }
    let mut data = b"PACK".to_vec();
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for object in objects {
        let kind = match object.kind {
            ObjectKind::Commit => 1,
            ObjectKind::Tree => 2,
            ObjectKind::Blob => 3,
            ObjectKind::Tag => 4,
        };
        let mut size = object.data.len();
        let mut first = ((kind << 4) | (size & 15)) as u8;
        size >>= 4;
        if size != 0 {
            first |= 128;
        }
        data.push(first);
        while size != 0 {
            let mut byte = (size & 127) as u8;
            size >>= 7;
            if size != 0 {
                byte |= 128;
            }
            data.push(byte);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&object.data)?;
        data.extend_from_slice(&encoder.finish()?);
        if data.len() > MAX_PACK_BYTES {
            return Err(Error::Unsupported("compressed pack exceeds limit".into()));
        }
    }
    let checksum = repo.objects.hash.digest(&data)?;
    data.extend_from_slice(checksum.bytes());
    Ok(data)
}

pub fn request(
    repo: &Repository,
    head: &ObjectId,
    reference: &str,
    advertisement: &Advertisement,
) -> Result<Vec<u8>> {
    if !reference.starts_with("refs/heads/") || !valid_ref(reference) {
        return Err(Error::Unsupported(
            "only branch pushes are implemented".into(),
        ));
    }
    let old = advertisement.refs.get(reference);
    let zero = "0".repeat(repo.objects.hash.hex_len());
    let old_text = old.map(ToString::to_string).unwrap_or(zero);
    let mut capabilities = String::from("report-status");
    if advertisement
        .capabilities
        .contains(&format!("object-format={}", repo.objects.hash.name()))
    {
        capabilities.push_str(&format!(" object-format={}", repo.objects.hash.name()));
    }
    let command = format!("{old_text} {head} {reference}\0{capabilities}\n");
    let mut body = packet(command.as_bytes())?;
    body.extend_from_slice(b"0000");
    body.extend_from_slice(&pack(repo, head, old)?);
    Ok(body)
}

pub fn check_status(bytes: &[u8], reference: &str) -> Result<()> {
    let packets = read_packets(bytes)?;
    if packets.last() != Some(&None) {
        return Err(Error::Invalid("push status missing flush".into()));
    }
    let lines: Vec<_> = packets
        .iter()
        .filter_map(|p| p.as_ref())
        .map(|line| std::str::from_utf8(line).map(str::trim_end))
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| Error::Invalid("non-UTF-8 push status".into()))?;
    if lines.first() != Some(&"unpack ok") {
        return Err(Error::Invalid(format!(
            "remote unpack failed: {}",
            lines.first().unwrap_or(&"missing status")
        )));
    }
    if lines.len() != 2 || lines[1] != format!("ok {reference}") {
        return Err(Error::Invalid(format!(
            "remote rejected push: {}",
            lines.get(1).unwrap_or(&"missing ref status")
        )));
    }
    Ok(())
}

/// Tokens live only in memory and Authorization headers; redirects and URL
/// credentials are prohibited so they cannot leak to another origin or logs.
pub fn https(
    repo: &Repository,
    remote: &str,
    source: &str,
    destination: &str,
    token: &str,
) -> Result<ObjectId> {
    let url = Url::parse(remote).map_err(|_| Error::Invalid("invalid remote URL".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
    {
        return Err(Error::Unsupported(
            "HTTPS pushes currently require a credential-free github.com URL".into(),
        ));
    }
    let segments: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if segments.len() != 2
        || segments.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err(Error::Invalid(
            "expected https://github.com/owner/repository.git".into(),
        ));
    }
    let hooks = repo
        .config
        .get("core.hookspath")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| repo.git_dir.join("hooks"));
    if hooks.join("pre-push").exists() {
        return Err(Error::Unsupported("pre-push hook".into()));
    }
    if token.is_empty() {
        return Err(Error::Invalid(
            "HTTPS authentication requires RUGIT_TOKEN".into(),
        ));
    }
    let head = repo.resolve(source)?;
    if repo.objects.read(&head)?.kind != ObjectKind::Commit {
        return Err(Error::Invalid("push source is not a commit".into()));
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(Policy::none())
        .build()
        .map_err(|_| Error::Invalid("could not initialize HTTPS client".into()))?;
    let base = remote.trim_end_matches('/');
    let response = client
        .get(format!("{base}/info/refs?service=git-receive-pack"))
        .basic_auth("x-access-token", Some(token))
        .send()
        .map_err(|_| Error::Invalid("HTTPS reference discovery failed".into()))?;
    let advertisement = Advertisement::parse(
        &response_bytes(response, "application/x-git-receive-pack-advertisement")?,
        repo.objects.hash,
    )?;
    let body = request(repo, &head, destination, &advertisement)?;
    let response = client
        .post(format!("{base}/git-receive-pack"))
        .basic_auth("x-access-token", Some(token))
        .header("Content-Type", "application/x-git-receive-pack-request")
        .header("Accept", "application/x-git-receive-pack-result")
        .body(body)
        .send()
        .map_err(|_| {
            Error::Invalid(
                "HTTPS pack upload failed; inspect the remote ref before retrying".into(),
            )
        })?;
    check_status(
        &response_bytes(response, "application/x-git-receive-pack-result")?,
        destination,
    )?;
    Ok(head)
}

fn response_bytes(response: reqwest::blocking::Response, expected: &str) -> Result<Vec<u8>> {
    if !response.status().is_success() {
        return Err(Error::Invalid(format!(
            "Git HTTP request failed: {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("");
    if content_type != expected {
        return Err(Error::Invalid("unexpected Git HTTP content type".into()));
    }
    read_bounded(response, 8 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_packets_fail_closed() {
        for bytes in [b"000".as_slice(), b"zzzz", b"0003", b"0008abc", b"ffff"] {
            assert!(read_packets(bytes).is_err());
        }
        assert_eq!(
            read_packets(b"0007abc0000").unwrap(),
            vec![Some(b"abc".as_slice()), None]
        );
    }
    #[test]
    fn remote_status_must_confirm_unpack_and_the_requested_branch() {
        let mut good = packet(b"unpack ok\n").unwrap();
        good.extend(packet(b"ok refs/heads/main\n").unwrap());
        good.extend(b"0000");
        assert!(check_status(&good, "refs/heads/main").is_ok());
        assert!(check_status(&good, "refs/heads/other").is_err());
        let mut bad = packet(b"unpack ok\n").unwrap();
        bad.extend(packet(b"ng refs/heads/main denied\n").unwrap());
        bad.extend(b"0000");
        assert!(check_status(&bad, "refs/heads/main").is_err());
        assert!(check_status(b"0000", "refs/heads/main").is_err());
    }
}

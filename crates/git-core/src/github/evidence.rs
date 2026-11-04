// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Read-only repository evidence and bounded source/release downloads.
use super::{api_body, api_client, validate_name};
use crate::{Error, Result};
use git_platform::TempFile;
use reqwest::{blocking::Response, header::LOCATION, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

fn artifact_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Invalid("could not initialize artifact HTTPS client".into()))
}

fn resource_path(full_name: &str, resource: &str) -> Result<String> {
    validate_name(full_name)?;
    if resource.is_empty()
        || resource.len() > 4096
        || !resource
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.?=&".contains(&b))
        || resource
            .split('?')
            .next()
            .unwrap_or_default()
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Error::Invalid("invalid repository API resource".into()));
    }
    Ok(format!(
        "https://api.github.com/repos/{full_name}/{resource}"
    ))
}

pub fn api(full_name: &str, resource: &str, token: &str) -> Result<Value> {
    let client = api_client()?;
    let mut request = client
        .get(resource_path(full_name, resource)?)
        .header("User-Agent", "ferragit/0.1.0")
        .header("Accept", "application/vnd.github+json");
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    api_body(
        request
            .send()
            .map_err(|_| Error::Invalid("GitHub repository evidence request failed".into()))?,
        200,
    )
}

fn redirected(
    response: Response,
    source_archive: bool,
    actions_artifact: bool,
) -> Result<Response> {
    if response.status().as_u16() != 302 {
        return Ok(response);
    }
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 16 * 1024)
        .ok_or_else(|| Error::Invalid("missing GitHub artifact destination".into()))?;
    let url = if actions_artifact {
        super::logs::download_url(location)?
    } else {
        Url::parse(location)
            .map_err(|_| Error::Invalid("invalid GitHub artifact destination".into()))?
    };
    let host = if source_archive {
        "codeload.github.com"
    } else {
        "release-assets.githubusercontent.com"
    };
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || (!actions_artifact && url.host_str() != Some(host))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Unsupported(
            "untrusted GitHub artifact destination".into(),
        ));
    }
    // The signed storage URL and repository credential never leave this function.
    artifact_client()?
        .get(url)
        .header("User-Agent", "ferragit/0.1.0")
        .send()
        .map_err(|_| Error::Invalid("GitHub artifact download failed".into()))
}

pub fn download(full_name: &str, resource: &str, destination: &Path, token: &str) -> Result<Value> {
    let source_archive = resource
        .strip_prefix("tarball/")
        .is_some_and(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()));
    let release_asset = resource
        .strip_prefix("releases/assets/")
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    let actions_artifact = resource
        .strip_prefix("actions/artifacts/")
        .and_then(|s| s.strip_suffix("/zip"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    if !source_archive && !release_asset && !actions_artifact {
        return Err(Error::Unsupported(
            "download requires tarball/COMMIT, releases/assets/ID or actions/artifacts/ID/zip"
                .into(),
        ));
    }
    if destination.exists() {
        return Err(Error::Invalid("artifact destination already exists".into()));
    }
    let client = artifact_client()?;
    let mut request = client
        .get(resource_path(full_name, resource)?)
        .header("User-Agent", "ferragit/0.1.0")
        .header(
            "Accept",
            if source_archive || actions_artifact {
                "application/vnd.github+json"
            } else {
                "application/octet-stream"
            },
        );
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    let mut response = redirected(
        request
            .send()
            .map_err(|_| Error::Invalid("GitHub artifact request failed".into()))?,
        source_archive,
        actions_artifact,
    )?;
    if response.status().as_u16() != 200 {
        return Err(Error::Invalid(format!(
            "GitHub artifact returned HTTP {}",
            response.status()
        )));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = TempFile::in_directory(parent)?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_DOWNLOAD {
            return Err(Error::Unsupported("artifact exceeds 512 MiB".into()));
        }
        hash.update(&buffer[..count]);
        temporary.writer().write_all(&buffer[..count])?;
    }
    let digest = format!("{:x}", hash.finalize());
    temporary.persist_no_clobber(destination)?;
    let mut published = fs::File::open(destination)?;
    let mut verification = Sha256::new();
    let mut verified_bytes = 0u64;
    loop {
        let count = published.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        verified_bytes += count as u64;
        if verified_bytes > MAX_DOWNLOAD {
            return Err(Error::Unsupported(
                "published artifact exceeds 512 MiB".into(),
            ));
        }
        verification.update(&buffer[..count]);
    }
    let actual = format!("{:x}", verification.finalize());
    if actual != digest {
        return Err(Error::Invalid(
            "published artifact differs from downloaded content".into(),
        ));
    }
    Ok(json!({"repository":full_name,"resource":resource,"sha256":digest,"bytes":bytes}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_evidence_paths_cannot_escape_the_repository() {
        for bad in [
            "",
            "../user",
            "git/../user",
            "/releases",
            "git/%2e%2e/user",
            "releases#fragment",
            "https://example.invalid",
        ] {
            assert!(resource_path("owner/repo", bad).is_err());
        }
        assert!(resource_path("owner/repo", "releases/tags/v2.56.0.windows.4").is_ok());
        assert!(resource_path("owner/repo", "git/trees/main?recursive=1").is_ok());
    }
}

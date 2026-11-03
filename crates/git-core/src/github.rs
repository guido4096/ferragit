// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Remote provisioning is an explicit rugit extension, not a standard Git command.
//! GitHub's API creates an empty private repository; push transfers the Git objects.
use crate::{store::read_bounded, Error, Result};
use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use serde_json::{json, Value};
use std::time::Duration;

fn api_body(response: Response, expected: u16) -> Result<Value> {
    if response.status().as_u16() != expected {
        return Err(Error::Invalid(format!(
            "GitHub repository API returned HTTP {}",
            response.status()
        )));
    }
    let bytes = read_bounded(response, 1024 * 1024)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::Invalid("invalid GitHub repository API response".into()))
}

pub fn validate_name(full_name: &str) -> Result<(&str, &str)> {
    let (owner, name) = full_name
        .split_once('/')
        .ok_or_else(|| Error::Invalid("repository must be OWNER/NAME".into()))?;
    if owner.is_empty()
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || name.is_empty()
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Error::Invalid(
            "invalid GitHub repository owner/name".into(),
        ));
    }
    Ok((owner, name))
}

pub fn create_private(full_name: &str, token: &str) -> Result<String> {
    let (owner, name) = validate_name(full_name)?;
    if token.is_empty() {
        return Err(Error::Invalid(
            "repository creation requires RUGIT_TOKEN".into(),
        ));
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .build()
        .map_err(|_| Error::Invalid("could not initialize GitHub HTTPS client".into()))?;
    let profile = client
        .get("https://api.github.com/user")
        .bearer_auth(token)
        .header("User-Agent", "rugit/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| Error::Invalid("GitHub identity request failed".into()))?;
    let profile = api_body(profile, 200)?;
    if !profile["login"]
        .as_str()
        .is_some_and(|login| login.eq_ignore_ascii_case(owner))
    {
        return Err(Error::Invalid(
            "authenticated account differs from requested repository owner".into(),
        ));
    }
    let response=client.post("https://api.github.com/user/repos").bearer_auth(token)
        .header("User-Agent","rugit/0.1.0").header("Accept","application/vnd.github+json").header("Content-Type","application/json")
        .body(json!({"name":name,"private":true,"auto_init":false,"description":"Proof of concept: independent Rust Git implementation; not production-ready"}).to_string())
        .send().map_err(|_|Error::Invalid("GitHub repository creation request failed; inspect your account before retrying".into()))?;
    // HTTP 422 on a name collision is an error, never permission to reuse a repo.
    let created = api_body(response, 201)?;
    if created["private"] != true
        || !created["full_name"]
            .as_str()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(full_name))
    {
        return Err(Error::Invalid(
            "created repository does not match the requested private destination".into(),
        ));
    }
    Ok(format!("https://github.com/{full_name}"))
}

fn get(path: &str, token: &str) -> Result<Value> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .build()
        .map_err(|_| Error::Invalid("could not initialize GitHub HTTPS client".into()))?;
    let response = client
        .get(format!("https://api.github.com{path}"))
        .bearer_auth(token)
        .header("User-Agent", "rugit/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| Error::Invalid("GitHub verification request failed".into()))?;
    api_body(response, 200)
}

pub fn inspect(full_name: &str, token: &str) -> Result<Value> {
    validate_name(full_name)?;
    let repo = get(&format!("/repos/{full_name}"), token)?;
    Ok(
        json!({"id":repo["id"],"full_name":repo["full_name"],"private":repo["private"],"html_url":repo["html_url"],"clone_url":repo["clone_url"]}),
    )
}

/// Independently compare GitHub's ref and blob IDs/modes with the local commit
/// and staged snapshot, without executing Git or downloading arbitrary contents.
pub fn verify(repo: &crate::repository::Repository, full_name: &str, token: &str) -> Result<Value> {
    use std::collections::BTreeMap;
    validate_name(full_name)?;
    let metadata = inspect(full_name, token)?;
    if !metadata["private"].is_boolean() {
        return Err(Error::Invalid(
            "missing GitHub repository visibility".into(),
        ));
    }
    let head = repo.resolve("HEAD")?;
    let target = repo.head_target()?;
    if target != "refs/heads/main" {
        return Err(Error::Unsupported(
            "bootstrap verification currently requires main".into(),
        ));
    }
    let remote = get(&format!("/repos/{full_name}/git/ref/heads/main"), token)?;
    if remote["object"]["sha"].as_str() != Some(&head.to_string()) {
        return Err(Error::Invalid("GitHub ref differs from local HEAD".into()));
    }
    let commit = get(&format!("/repos/{full_name}/git/commits/{head}"), token)?;
    let tree = commit["tree"]["sha"]
        .as_str()
        .ok_or_else(|| Error::Invalid("missing remote commit tree".into()))?;
    let raw = repo.objects.read(&head)?;
    if !raw.data.starts_with(format!("tree {tree}\n").as_bytes()) {
        return Err(Error::Invalid(
            "GitHub tree differs from local commit".into(),
        ));
    }
    let tree = get(
        &format!("/repos/{full_name}/git/trees/{tree}?recursive=1"),
        token,
    )?;
    if tree["truncated"] != false {
        return Err(Error::Invalid("truncated remote tree".into()));
    }
    let mut remote_files = BTreeMap::new();
    for item in tree["tree"]
        .as_array()
        .ok_or_else(|| Error::Invalid("invalid remote tree".into()))?
    {
        if item["type"] == "tree" {
            continue;
        }
        if item["type"] != "blob" {
            return Err(Error::Unsupported("non-blob remote tree entry".into()));
        }
        let value = |key: &str| {
            item[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Invalid("invalid remote tree entry".into()))
        };
        remote_files.insert(value("path")?, (value("mode")?, value("sha")?));
    }
    let mut local_files = BTreeMap::new();
    for entry in crate::index::read(repo)? {
        let mode = format!("{:06o}", entry.mode());
        let path = String::from_utf8(entry.path)
            .map_err(|_| Error::Unsupported("GitHub API verification of non-UTF-8 paths".into()))?;
        local_files.insert(path, (mode, entry.id.to_string()));
    }
    if remote_files != local_files {
        return Err(Error::Invalid(
            "GitHub blob IDs/modes differ from the local index".into(),
        ));
    }
    Ok(
        json!({"repository":full_name,"private":metadata["private"],"url":metadata["html_url"],"commit":head.to_string(),"remote_commit":remote["object"]["sha"],"verified_file_count":local_files.len()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_names_cannot_change_the_api_endpoint() {
        for value in [
            "",
            "owner",
            "owner/../repo",
            "owner/..",
            "owner/repo?private=false",
            "owner/repo#fragment",
            "owner/repo\n",
        ] {
            assert!(validate_name(value).is_err());
        }
        assert_eq!(
            validate_name("guido4096/rugit").unwrap(),
            ("guido4096", "rugit")
        );
    }
}

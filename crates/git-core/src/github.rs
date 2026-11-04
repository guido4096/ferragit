// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Remote provisioning is an explicit ferragit extension, not a standard Git command.
//! GitHub's API creates an empty private repository; push transfers the Git objects.
mod evidence;
pub use evidence::{api, download};
mod logs;
pub use logs::job_log;

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
            "repository creation requires FERRAGIT_TOKEN".into(),
        ));
    }
    let client = api_client()?;
    let profile = client
        .get("https://api.github.com/user")
        .bearer_auth(token)
        .header("User-Agent", "ferragit/0.1.0")
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
        .header("User-Agent","ferragit/0.1.0").header("Accept","application/vnd.github+json").header("Content-Type","application/json")
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

fn api_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .build()
        .map_err(|_| Error::Invalid("could not initialize GitHub HTTPS client".into()))
}

fn get(path: &str, token: &str) -> Result<Value> {
    let response = api_client()?
        .get(format!("https://api.github.com{path}"))
        .bearer_auth(token)
        .header("User-Agent", "ferragit/0.1.0")
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

/// Change only the repository name, preserving its identity and visibility.
pub fn rename(full_name: &str, new_name: &str, token: &str) -> Result<Value> {
    let (owner, _) = validate_name(full_name)?;
    let destination = format!("{owner}/{new_name}");
    validate_name(&destination)?;
    let before = inspect(full_name, token)?;
    if before["id"].as_u64().is_none() || !before["private"].is_boolean() {
        return Err(Error::Invalid("invalid GitHub repository identity".into()));
    }
    let response = api_client()?
        .patch(format!("https://api.github.com/repos/{full_name}"))
        .bearer_auth(token)
        .header("User-Agent", "ferragit/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(json!({"name":new_name}).to_string())
        .send()
        .map_err(|_| {
            Error::Invalid(
                "GitHub repository update request failed; inspect the destination before retrying"
                    .into(),
            )
        })?;
    let updated = api_body(response, 200)?;
    validate_renamed_repository(&before, &updated, &destination)?;
    let verified = inspect(&destination, token)?;
    validate_renamed_repository(&before, &verified, &destination)?;
    Ok(verified)
}

fn validate_renamed_repository(before: &Value, updated: &Value, destination: &str) -> Result<()> {
    if before["id"].as_u64().is_none()
        || !before["private"].is_boolean()
        || updated["id"] != before["id"]
        || updated["private"] != before["private"]
        || !updated["full_name"]
            .as_str()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(destination))
    {
        return Err(Error::Invalid(
            "updated GitHub repository differs from the requested identity or visibility".into(),
        ));
    }
    Ok(())
}

/// Read the CI checks attached to the local commit without executing another client.
pub fn checks(repo: &crate::repository::Repository, full_name: &str, token: &str) -> Result<Value> {
    validate_name(full_name)?;
    let head = repo.resolve("HEAD")?;
    let response = get(
        &format!("/repos/{full_name}/commits/{head}/check-runs?per_page=100"),
        token,
    )?;
    let runs = response["check_runs"]
        .as_array()
        .ok_or_else(|| Error::Invalid("invalid GitHub checks response".into()))?;
    if response["total_count"].as_u64() != Some(runs.len() as u64) {
        return Err(Error::Unsupported("more than 100 GitHub checks".into()));
    }
    let mut checks = Vec::new();
    for run in runs {
        let mut check = json!({"name":run["name"],"status":run["status"],"conclusion":run["conclusion"],"url":run["html_url"]});
        if run["conclusion"] == "failure" {
            let id = run["id"]
                .as_u64()
                .ok_or_else(|| Error::Invalid("invalid GitHub check ID".into()))?;
            check["annotations"] = get(
                &format!("/repos/{full_name}/check-runs/{id}/annotations?per_page=100"),
                token,
            )?;
            check["output"] = json!({"title":run["output"]["title"],"summary":run["output"]["summary"],"text":run["output"]["text"]});
            let prefix = format!("https://github.com/{full_name}/actions/runs/");
            if let Some((_, job)) = run["html_url"]
                .as_str()
                .and_then(|url| url.strip_prefix(&prefix))
                .and_then(|rest| rest.split_once("/job/"))
            {
                if let Ok(job) = job.parse::<u64>() {
                    let result = get(&format!("/repos/{full_name}/actions/jobs/{job}"), token)?;
                    check["steps"] = result["steps"].clone();
                }
            }
        }
        checks.push(check);
    }
    Ok(json!({"repository":full_name,"commit":head.to_string(),"checks":checks}))
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
    fn repository_updates_preserve_identity_and_visibility() {
        let before = json!({"id":42,"private":false,"full_name":"owner/source"});
        let updated = json!({"id":42,"private":false,"full_name":"owner/destination"});
        assert!(validate_renamed_repository(&before, &updated, "owner/destination").is_ok());
        for (key, value) in [
            ("id", json!(43)),
            ("private", json!(true)),
            ("full_name", json!("other/destination")),
        ] {
            let mut invalid = updated.clone();
            invalid[key] = value;
            assert!(validate_renamed_repository(&before, &invalid, "owner/destination").is_err());
        }
        assert!(validate_renamed_repository(&json!({}), &json!({}), "owner/destination").is_err());
    }
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
            validate_name("guido4096/ferragit").unwrap(),
            ("guido4096", "ferragit")
        );
    }
}

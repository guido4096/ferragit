// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Bounded Actions log retrieval. The GitHub credential is used only on the API
//! request; the short-lived download URL is neither printed nor authenticated.
use super::{api_client, validate_name};
use crate::{store::read_bounded, Error, Result};
use reqwest::{header::LOCATION, Url};

const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn download_url(location: &str) -> Result<Url> {
    let url = Url::parse(location)
        .map_err(|_| Error::Invalid("invalid GitHub log download destination".into()))?;
    let trusted_host = url.host_str().is_some_and(|host| {
        host.ends_with(".blob.core.windows.net") || host.ends_with(".actions.githubusercontent.com")
    });
    if location.len() > 16 * 1024
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !trusted_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Unsupported(
            "untrusted GitHub log download destination".into(),
        ));
    }
    Ok(url)
}

pub fn job_log(full_name: &str, job: u64, token: &str) -> Result<Vec<u8>> {
    validate_name(full_name)?;
    let client = api_client()?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{full_name}/actions/jobs/{job}/logs"
        ))
        .bearer_auth(token)
        .header("User-Agent", "ferragit/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| Error::Invalid("GitHub job log request failed".into()))?;
    let response = if response.status().as_u16() == 302 {
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Error::Invalid("missing GitHub log download destination".into()))?;
        // Authorization is deliberately absent from this separate request.
        client
            .get(download_url(location)?)
            .header("User-Agent", "ferragit/0.1.0")
            .send()
            .map_err(|_| Error::Invalid("GitHub job log download failed".into()))?
    } else {
        response
    };
    if response.status().as_u16() != 200 {
        return Err(Error::Invalid(format!(
            "GitHub job log request returned HTTP {}",
            response.status()
        )));
    }
    read_bounded(response, MAX_LOG_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_download_destinations_are_https_and_restricted_to_actions_storage() {
        for valid in [
            "https://logs.blob.core.windows.net/job?sig=example",
            "https://pipelines.actions.githubusercontent.com/job",
        ] {
            assert!(download_url(valid).is_ok());
        }
        for invalid in [
            "http://logs.blob.core.windows.net/job",
            "https://logs.blob.core.windows.net.evil.invalid/job",
            "https://example.invalid/job",
            "https://user@logs.blob.core.windows.net/job",
            "https://logs.blob.core.windows.net:8443/job",
            "https://logs.blob.core.windows.net/job#fragment",
            "not a URL",
        ] {
            assert!(download_url(invalid).is_err(), "accepted {invalid}");
        }
    }
}

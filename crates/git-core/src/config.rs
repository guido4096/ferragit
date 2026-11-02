// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! A deliberately limited config reader. Reject syntax we cannot interpret safely.
use crate::{object::HashKind, Error, Result};
use std::{fs, path::Path};

#[derive(Debug, Default, Clone)]
pub struct Config {
    entries: Vec<(String, String)>,
}

impl Config {
    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path)?)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut config = Self::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') {
                let end = line
                    .find(']')
                    .ok_or_else(|| Error::Invalid("bad config section".into()))?;
                let tail = line[end + 1..].trim();
                if !tail.is_empty() && !tail.starts_with('#') && !tail.starts_with(';') {
                    return Err(Error::Invalid("bad config section suffix".into()));
                }
                let body = &line[1..end];
                section = if let Some((name, subsection)) = body.split_once(' ') {
                    let subsection = subsection.trim();
                    if !subsection.starts_with('"')
                        || !subsection.ends_with('"')
                        || subsection[1..subsection.len() - 1].contains(['"', '\\'])
                    {
                        return Err(Error::Unsupported("escaped config subsection".into()));
                    }
                    format!(
                        "{}.{}",
                        name.to_ascii_lowercase(),
                        &subsection[1..subsection.len() - 1]
                    )
                } else {
                    body.to_ascii_lowercase()
                };
                continue;
            }
            if section.is_empty() {
                return Err(Error::Invalid("config entry outside section".into()));
            }
            let (key, raw) = line
                .split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
                .unwrap_or((line, "true"));
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err(Error::Invalid("bad config key".into()));
            }
            let value = parse_value(raw)?;
            let key = format!("{}.{}", section, key.to_ascii_lowercase());
            if key.starts_with("include.") || key.starts_with("includeif.") {
                return Err(Error::Unsupported("config includes".into()));
            }
            config.entries.push((key, value));
        }
        Ok(config)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn repository_hash(&self) -> Result<HashKind> {
        let version = self
            .get("core.repositoryformatversion")
            .unwrap_or("0")
            .parse::<u32>()
            .map_err(|_| Error::Invalid("invalid repository format version".into()))?;
        if version > 1 {
            return Err(Error::Unsupported(format!(
                "repository format version {version}"
            )));
        }
        for (key, _) in &self.entries {
            if key.starts_with("extensions.") && key != "extensions.objectformat" {
                return Err(Error::Unsupported(format!("repository extension '{key}'")));
            }
        }
        let hash = HashKind::parse(self.get("extensions.objectformat").unwrap_or("sha1"))?;
        if hash == HashKind::Sha256 && version != 1 {
            return Err(Error::Invalid(
                "SHA-256 requires repository format version 1".into(),
            ));
        }
        Ok(hash)
    }

    pub fn boolean(&self, key: &str, default: bool) -> Result<bool> {
        match self.get(key).map(str::to_ascii_lowercase).as_deref() {
            None => Ok(default),
            Some("true" | "yes" | "on" | "1" | "") => Ok(true),
            Some("false" | "no" | "off" | "0") => Ok(false),
            _ => Err(Error::Invalid(format!(
                "bad boolean config value for {key}"
            ))),
        }
    }
}

fn parse_value(raw: &str) -> Result<String> {
    let mut quoted = false;
    let mut value = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            '\\' => value.push(match chars.next() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('b') => '\u{8}',
                Some('"') => '"',
                Some('\\') => '\\',
                _ => return Err(Error::Unsupported("config escape or continuation".into())),
            }),
            _ => value.push(c),
        }
    }
    if quoted {
        return Err(Error::Invalid("unterminated config quote".into()));
    }
    Ok(value.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extensions_are_checked_before_repository_use() {
        assert!(Config::parse("[core]\nrepositoryformatversion=999")
            .unwrap()
            .repository_hash()
            .is_err());
        assert!(Config::parse(
            "[core]\nrepositoryformatversion=1\n[extensions]\nrefStorage=reftable"
        )
        .unwrap()
        .repository_hash()
        .is_err());
        assert_eq!(
            Config::parse("[core]\nrepositoryformatversion=1\n[extensions]\nobjectFormat=sha256")
                .unwrap()
                .repository_hash()
                .unwrap(),
            HashKind::Sha256
        );
    }
    #[test]
    fn quoted_comments_and_duplicate_precedence() {
        let config = Config::parse("[user]\nname=old\nname=\"new # name\" # comment\n[remote \"origin\"]\nurl=https://example.com/repo.git").unwrap();
        assert_eq!(config.get("user.name"), Some("new # name"));
        assert_eq!(
            config.get("remote.origin.url"),
            Some("https://example.com/repo.git")
        );
        assert!(Config::parse("[include]\npath=other").is_err());
    }
}

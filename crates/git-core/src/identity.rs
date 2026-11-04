// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Validated Git author/committer identity framing.
use crate::{Error, Result};

#[derive(Clone)]
pub struct Identity {
    pub name: String,
    pub email: String,
    pub date: String,
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.email.is_empty()
            || self.name.contains(['\n', '\r', '\0', '<', '>'])
            || self.email.contains(['\n', '\r', '\0', '<', '>'])
        {
            return Err(Error::Invalid("invalid author/committer identity".into()));
        }
        let Some((seconds, zone)) = self.date.split_once(' ') else {
            return Err(Error::Unsupported(
                "date format; use '<unix-seconds> +HHMM'".into(),
            ));
        };
        if seconds.parse::<u64>().is_err()
            || zone.len() != 5
            || !matches!(zone.as_bytes()[0], b'+' | b'-')
            || !zone.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(Error::Invalid("invalid identity date".into()));
        }
        Ok(())
    }
    pub(crate) fn line(&self) -> String {
        format!("{} <{}> {}", self.name, self.email, self.date)
    }
}

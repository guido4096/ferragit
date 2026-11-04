// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Git formats and repository operations, independent of CLI parsing and output.
pub mod commit;
pub mod config;
pub mod github;
pub mod identity;
pub mod index;
pub mod object;
pub mod push;
pub mod refs;
pub mod repository;
pub mod store;

use std::{fmt, io};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(String),
    Unsupported(String),
    NotRepository,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Unsupported(message) => {
                write!(formatter, "unsupported in this milestone: {message}")
            }
            Self::NotRepository => {
                formatter.write_str("not a git repository (or any of the parent directories): .git")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

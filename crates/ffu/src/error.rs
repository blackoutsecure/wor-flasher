use std::io;

/// Stable categories shared by library callers and the original helper CLI.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Source(String),
    #[error("{0}")]
    Arguments(String),
    #[error("{0}")]
    Target(String),
    #[error("{0}")]
    Apply(String),
    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("interrupted by signal {signal}")]
    Interrupted { signal: i32 },
}

impl Error {
    pub fn exit_status(&self) -> i32 {
        match self {
            Self::Source(_) | Self::Arguments(_) => 2,
            Self::Target(_) => 3,
            Self::Apply(_) | Self::Io { .. } => 4,
            Self::Interrupted { signal } => 128 + signal,
        }
    }

    pub(crate) fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

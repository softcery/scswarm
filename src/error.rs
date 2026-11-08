use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Exit 1: config or board rejected.
    Invalid(String),
    /// Exit 2: host call failed.
    Failed(String),
}

/// Exit codes, also read back from workers.
pub const OK: i32 = 0;
pub const INVALID: i32 = 1;
pub const FAILED: i32 = 2;
pub const STOPPED: i32 = 3;

impl Error {
    pub fn code(&self) -> i32 {
        match self {
            Error::Invalid(_) => INVALID,
            Error::Failed(_) => FAILED,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(what) | Error::Failed(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {}

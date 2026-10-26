use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Exit 1: config or board rejected.
    Invalid(String),
    /// Exit 2: host call failed.
    Failed(String),
}

impl Error {
    pub fn code(&self) -> i32 {
        match self {
            Error::Invalid(_) => 1,
            Error::Failed(_) => 2,
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

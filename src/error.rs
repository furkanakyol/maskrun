use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    User(String),
    // main() must exit 0 silently here (Python: `except BrokenPipeError: return 0`).
    BrokenPipe,
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Error::User(text.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::User(text) => write!(f, "{text}"),
            Error::BrokenPipe => write!(f, "broken pipe"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::BrokenPipe {
            return Error::BrokenPipe;
        }
        Error::User(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::User(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

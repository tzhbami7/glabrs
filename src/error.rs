use std::fmt;

/// Script-friendly exit codes, documented in the README.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    Runtime = 1,
    Usage = 2,
    Auth = 3,
    NotFound = 4,
}

#[derive(Debug)]
pub struct Error {
    message: String,
    exit: Exit,
}

impl Error {
    pub fn runtime(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit: Exit::Runtime,
        }
    }
    pub fn auth(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit: Exit::Auth,
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit: Exit::NotFound,
        }
    }
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit: Exit::Usage,
        }
    }
    pub fn exit_code(&self) -> u8 {
        self.exit as u8
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        // Never include response bodies in messages: they can echo tokens.
        match &e {
            ureq::Error::Status(code, _) if *code == 401 => {
                Error::auth("HTTP 401 Unauthorized: token missing, invalid, or insufficient")
            }
            _ => Error::runtime(format!("HTTP error: {e}")),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::runtime(format!("I/O error: {e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::runtime(format!("JSON error: {e}"))
    }
}

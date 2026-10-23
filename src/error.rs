use std::fmt;

/// Script-friendly exit codes, documented in the README.
///
/// Small `Copy` enum passed by value ([`Exit::as_u8`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// Runtime error (the default failure class).
    Runtime = 1,
    /// Usage error (invalid arguments or flag combinations).
    Usage = 2,
    /// Auth invalid / missing.
    Auth = 3,
    /// Empty result / not found.
    NotFound = 4,
}

impl Exit {
    /// The numeric exit code for scripting (`0` is success and never produced
    /// by an [`Error`]).
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Crate-level error carrying a script-relevant exit code.
///
/// Variants map 1:1 to [`Exit`] codes; the `Display` string is what the CLI
/// prints to stderr. Messages never include HTTP response bodies (they can
/// echo tokens — see the `From<ureq::Error>` impl).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Runtime failure (exit code 1): I/O, HTTP, git, unexpected API shapes.
    #[error("{0}")]
    Runtime(String),
    /// Usage error (exit code 2): invalid flag combinations or argument forms.
    #[error("{0}")]
    Usage(String),
    /// Auth failure (exit code 3): missing, invalid, or insufficient token.
    #[error("{0}")]
    Auth(String),
    /// Empty result / not found (exit code 4).
    #[error("{0}")]
    NotFound(String),
}

impl Error {
    /// Runtime failure (exit code 1).
    pub fn runtime(msg: impl Into<String>) -> Self {
        Self::Runtime(msg.into())
    }
    /// Auth failure (exit code 3).
    pub fn auth(msg: impl Into<String>) -> Self {
        Self::Auth(msg.into())
    }
    /// Empty result / not found (exit code 4).
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    /// Usage error (exit code 2).
    pub fn usage(msg: impl Into<String>) -> Self {
        Self::Usage(msg.into())
    }

    /// The process exit code this error maps to.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Runtime(_) => Exit::Runtime.as_u8(),
            Self::Usage(_) => Exit::Usage.as_u8(),
            Self::Auth(_) => Exit::Auth.as_u8(),
            Self::NotFound(_) => Exit::NotFound.as_u8(),
        }
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_u8())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_map_to_exit_codes() {
        assert_eq!(Error::runtime("x").exit_code(), 1);
        assert_eq!(Error::usage("x").exit_code(), 2);
        assert_eq!(Error::auth("x").exit_code(), 3);
        assert_eq!(Error::not_found("x").exit_code(), 4);
    }

    #[test]
    fn display_is_the_message() {
        assert_eq!(Error::runtime("disk full").to_string(), "disk full");
        assert_eq!(Error::usage("bad flag").to_string(), "bad flag");
        assert_eq!(Error::auth("no token").to_string(), "no token");
        assert_eq!(Error::not_found("gone").to_string(), "gone");
    }

    #[test]
    fn exit_enum_values_are_stable() {
        // Pinned: scripts depend on these numbers (README "Exit codes").
        assert_eq!(Exit::Runtime.as_u8(), 1);
        assert_eq!(Exit::Usage.as_u8(), 2);
        assert_eq!(Exit::Auth.as_u8(), 3);
        assert_eq!(Exit::NotFound.as_u8(), 4);
    }

    #[test]
    fn http_401_maps_to_auth_and_other_statuses_to_runtime() {
        // 401 is an auth failure (exit 3); every other HTTP error is runtime
        // (exit 1). Built from real ureq::Error values.
        let unauthorized =
            ureq::Error::Status(401, ureq::Response::new(401, "Unauthorized", "").unwrap());
        let err = Error::from(unauthorized);
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            err.to_string(),
            "HTTP 401 Unauthorized: token missing, invalid, or insufficient"
        );

        let not_found =
            ureq::Error::Status(404, ureq::Response::new(404, "Not Found", "").unwrap());
        let err = Error::from(not_found);
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().starts_with("HTTP error: "));
    }

    #[test]
    fn io_and_json_errors_map_to_runtime_with_prefixes() {
        let err = Error::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.to_string(), "I/O error: no such file");

        let err = Error::from(serde_json::from_str::<serde_json::Value>("{bad").unwrap_err());
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().starts_with("JSON error: "));
    }

    #[test]
    fn is_std_error_and_has_no_source() {
        // thiserror provides the std::error::Error impl; string payloads carry
        // the full context, so there is no separate source chain.
        fn assert_error<E: std::error::Error>(_: &E) {}
        let err = Error::runtime("boom");
        assert_error(&err);
        assert!(std::error::Error::source(&err).is_none());
    }
}

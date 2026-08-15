use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    Config,
    Io,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    pub msg: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl Error {
    pub fn config(msg: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Config,
            msg: msg.into(),
            line: None,
            col: None,
        }
    }

    pub fn config_at(msg: impl Into<String>, line: usize, col: usize) -> Self {
        Error {
            kind: ErrorKind::Config,
            msg: msg.into(),
            line: Some(line),
            col: Some(col),
        }
    }

    pub fn io(msg: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Io,
            msg: msg.into(),
            line: None,
            col: None,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Config => 2,
            ErrorKind::Io => 2,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.col) {
            (Some(l), Some(c)) => write!(f, "{}:{}: {}", l, c, self.msg),
            _ => write!(f, "{}", self.msg),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::io(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::config(format!("JSON 错误：{e}"))
    }
}

impl From<lexopt::Error> for Error {
    fn from(e: lexopt::Error) -> Self {
        Error::config(e.to_string())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

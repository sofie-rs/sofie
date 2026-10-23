use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct ModelError {
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
}

impl ModelError {
    pub fn new(file: &Path, line: usize, message: impl Into<String>) -> Self {
        Self {
            file: file.to_path_buf(),
            line,
            message: message.into(),
        }
    }

    pub fn in_file(file: &Path, message: impl Into<String>) -> Self {
        Self::new(file, 1, message)
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file.display(), self.line, self.message)
    }
}

impl std::error::Error for ModelError {}

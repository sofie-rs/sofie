use std::fmt;

use crate::syntax::{LocatedError, Position};

#[derive(Debug, PartialEq, Eq)]
pub struct TemplateError {
    pub template: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl TemplateError {
    pub(crate) fn located(template: &str, error: LocatedError) -> Self {
        Self {
            template: template.to_owned(),
            line: error.position.line,
            column: error.position.column,
            message: error.message,
        }
    }

    pub(crate) fn unlocated(template: &str, message: impl Into<String>) -> Self {
        Self::located(template, LocatedError::new(Position::START, message))
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.template, self.line, self.column, self.message
        )
    }
}

impl std::error::Error for TemplateError {}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderError {
    FragmentNotFound { id: String },
    Template(TemplateError),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FragmentNotFound { id } => write!(
                f,
                "no element or component with id `{id}` is rendered for this data"
            ),
            Self::Template(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FragmentNotFound { .. } => None,
            Self::Template(error) => Some(error),
        }
    }
}

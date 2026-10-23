use std::fmt;

use crate::catalog::{Component, Kind};

/// Bounds recursion in the parser, the loader and the renderer alike.
pub(crate) const MAX_DEPTH: usize = 128;

#[cfg(any(feature = "parse", feature = "islands"))]
pub(crate) fn is_lowercase_name(name: &str) -> bool {
    name.bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub const START: Self = Self { line: 1, column: 1 };
}

impl fmt::Display for Position {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocatedError {
    pub position: Position,
    pub message: String,
}

impl LocatedError {
    pub fn new(position: Position, message: impl Into<String>) -> Self {
        Self {
            position,
            message: message.into(),
        }
    }
}

/// Markup is joined when parsing, so rendering writes only what comes from data.
/// A parsed or compiled template holds its components as [`Written`] until the
/// catalog builds them.
#[derive(Debug)]
pub(crate) enum Op<C = Component> {
    Markup(String),
    /// A value, escaped.
    Text(Expression),
    /// A URL attribute's value, checked for its scheme, then escaped.
    Url(Expression),
    /// Kept whole so a fragment request finds it.
    Identified {
        id: Expression,
        body: Vec<Op<C>>,
    },
    Component(C),
    If(Conditional<C>),
    Each(Loop<C>),
}

#[derive(Debug)]
pub(crate) struct Written {
    pub kind: Kind,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Op<Written>>,
    pub position: Position,
}

#[derive(Debug)]
pub(crate) struct Attribute {
    pub name: String,
    pub value: Expression,
    pub position: Position,
}

#[derive(Debug)]
pub(crate) struct Conditional<C = Component> {
    pub condition: Expression,
    pub then_branch: Vec<Op<C>>,
    pub else_branch: Vec<Op<C>>,
}

#[derive(Debug)]
pub(crate) struct Loop<C = Component> {
    pub list: Expression,
    pub has_index: bool,
    pub body: Vec<Op<C>>,
}

#[derive(Clone, Debug)]
pub(crate) enum Expression {
    Path(Path),
    /// A string literal in braces, `{"…"}`: data, so it is escaped.
    Text(String),
    Integer(i64),
    /// Quoted text as the author wrote it: HTML, written as is.
    Authored(String),
    Interpolated(Vec<Expression>),
    Call(Box<crate::language::Call>),
    #[cfg(feature = "gates")]
    Gate(crate::rendering::Gate),
}

#[derive(Clone, Debug)]
pub(crate) struct Path {
    pub root: Root,
    pub fields: Vec<String>,
}

/// Loop names are resolved to their binding slot at parse time, so rendering
/// never compares names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Root {
    Data,
    Slot(usize),
}

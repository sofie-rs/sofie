use std::fmt;

use super::cursor::Cursor;
use super::is_tag_name_byte;
use crate::syntax::{LocatedError, Position};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Closer {
    Tag(String),
    Else,
    EndIf,
    EndEach,
}

impl fmt::Display for Closer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tag(name) => write!(formatter, "`</{name}>`"),
            Self::Else => formatter.write_str("`{:else}`"),
            Self::EndIf => formatter.write_str("`{/if}`"),
            Self::EndEach => formatter.write_str("`{/each}`"),
        }
    }
}

pub(super) struct Opener {
    pub(super) description: String,
    pub(super) position: Position,
}

impl Opener {
    pub(super) fn new(description: impl Into<String>, position: Position) -> Self {
        Self {
            description: description.into(),
            position,
        }
    }
}

pub(super) fn is_at_closer(cursor: &Cursor<'_>) -> bool {
    ["</", "{:", "{/"]
        .iter()
        .any(|start| cursor.starts_with(start))
}

pub(super) fn close(
    cursor: &mut Cursor<'_>,
    opener: &Opener,
    accepted: &[Closer],
) -> Result<Closer, LocatedError> {
    let Some((closer, position)) = read_closer(cursor)? else {
        bail!(opener.position, "{} is never closed", opener.description);
    };
    if accepted.contains(&closer) {
        return Ok(closer);
    }
    bail!(
        position,
        "{closer} does not close {} opened at {}",
        opener.description,
        opener.position
    );
}

pub(super) fn read_closer(
    cursor: &mut Cursor<'_>,
) -> Result<Option<(Closer, Position)>, LocatedError> {
    let position = cursor.position();
    let closer = if cursor.eat("</") {
        let name = cursor.take_while(is_tag_name_byte);
        cursor.skip_whitespace();
        cursor.expect(">", &format!("to end `</{name}`"))?;
        Closer::Tag(name.to_owned())
    } else if cursor.eat("{:") {
        match read_clause_word(cursor)? {
            "else" => Closer::Else,
            word => return Err(unknown_clause(position, ":", word)),
        }
    } else if cursor.eat("{/") {
        match read_clause_word(cursor)? {
            "if" => Closer::EndIf,
            "each" => Closer::EndEach,
            word => return Err(unknown_clause(position, "/", word)),
        }
    } else {
        return Ok(None);
    };
    Ok(Some((closer, position)))
}

fn read_clause_word<'s>(cursor: &mut Cursor<'s>) -> Result<&'s str, LocatedError> {
    let word = cursor.take_identifier();
    cursor.skip_whitespace();
    cursor.expect("}", "to end the block tag")?;
    Ok(word)
}

fn unknown_clause(position: Position, sigil: &str, word: &str) -> LocatedError {
    LocatedError::new(
        position,
        format!(
            "unknown block tag `{{{sigil}{word}}}`; expected `{{:else}}`, `{{/if}}` or `{{/each}}`"
        ),
    )
}

use super::{Language, Message};
use crate::label::humanize;
use crate::message::Recorded;
use crate::syntax::{Expression, Position};

/// Never a value, so the parser accepts one only where text is written.
#[derive(Clone, Debug)]
#[cfg_attr(
    not(feature = "parse"),
    expect(dead_code, reason = "positions locate what the template check reports")
)]
pub(crate) enum Call {
    /// `t("key", name = value, …)`
    Translate {
        key: String,
        counted: Option<CountedKeys>,
        arguments: Vec<Argument>,
        recorded: Option<Recorded>,
        position: Position,
    },
    Format {
        format: Format,
        value: Expression,
        position: Position,
    },
    Label {
        scoped: Option<String>,
        key: String,
        humanized: String,
        position: Position,
    },
}

/// Built once at parse, as a counted call is looked up per row.
#[derive(Clone, Debug)]
pub(crate) struct CountedKeys {
    pub one: String,
    pub other: String,
}

impl Call {
    pub(crate) fn translate(
        key: String,
        arguments: Vec<Argument>,
        position: Position,
        recorded: Option<Recorded>,
    ) -> Self {
        let counted = arguments
            .iter()
            .any(|argument| argument.name == "count")
            .then(|| CountedKeys {
                one: format!("{key}.one"),
                other: format!("{key}.other"),
            });
        Self::Translate {
            key,
            counted,
            arguments,
            recorded,
            position,
        }
    }

    #[cfg(feature = "parse")]
    pub(crate) fn date(value: Expression, position: Position) -> Self {
        Self::Format {
            format: Format::Date,
            value,
            position,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Argument {
    pub name: String,
    pub value: Expression,
}

impl Argument {
    /// `number(…)` adds nothing here: a number argument is written localized anyway.
    pub(crate) fn plain_value(&self) -> &Expression {
        if let Expression::Call(call) = &self.value
            && let Call::Format {
                format: Format::Number,
                value,
                ..
            } = &**call
        {
            return value;
        }
        &self.value
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Format {
    Number,
    Money,
    Date,
}

pub(crate) fn label(name: &str, module: Option<&str>, position: Position) -> Expression {
    Expression::Call(Box::new(Call::Label {
        scoped: module.map(|module| module_label_key(module, name)),
        key: label_key(name),
        humanized: humanize(name),
        position,
    }))
}

/// A field's label in `module`'s own words, else the app-wide ones, else humanized.
#[must_use]
pub fn field_label(language: Language<'_>, module: &str, field: &str) -> String {
    let humanized = humanize(field);
    let scoped = module_label_key(module, field);
    label_text(language, Some(&scoped), &label_key(field), &humanized).to_owned()
}

/// The viewer's language comes first: its app-wide label beats the source language's module one.
pub(crate) fn label_text<'m>(
    language: Language<'m>,
    scoped: Option<&str>,
    key: &str,
    humanized: &'m str,
) -> &'m str {
    let keys = || scoped.into_iter().chain([key]);
    keys()
        .find_map(|key| language.own_message(key).and_then(Message::plain))
        .or_else(|| keys().find_map(|key| language.message(key).and_then(Message::plain)))
        .unwrap_or(humanized)
}

pub(crate) fn label_key(field: &str) -> String {
    format!("label.{field}")
}

pub(crate) fn module_label_key(module: &str, field: &str) -> String {
    format!("{module}.label.{field}")
}

#[cfg(feature = "parse")]
impl Call {
    /// The values a call reads, which a module component binds to where it is written.
    pub(crate) fn arguments_mut(&mut self) -> impl Iterator<Item = &mut Expression> {
        let (arguments, value): (&mut [Argument], Option<&mut Expression>) = match self {
            Self::Translate { arguments, .. } => (arguments, None),
            Self::Format { value, .. } => (&mut [], Some(value)),
            Self::Label { .. } => (&mut [], None),
        };
        arguments
            .iter_mut()
            .map(|argument| &mut argument.value)
            .chain(value)
    }

    pub(crate) fn recorded_mut(&mut self) -> Option<(&str, &mut Option<Recorded>)> {
        match self {
            Self::Translate { key, recorded, .. } => Some((key, recorded)),
            Self::Format { .. } | Self::Label { .. } => None,
        }
    }

    pub(crate) fn position(&self) -> Position {
        match self {
            Self::Translate { position, .. }
            | Self::Format { position, .. }
            | Self::Label { position, .. } => *position,
        }
    }
}

use std::marker::PhantomData;

use sofie_core::DisplayZone;
use sofie_values::{Instant, Money};

use crate::escape::push_escaped;
use crate::label::humanize;
use crate::message::Recorded;
use crate::render::push_display;
use crate::syntax::{Expression, Position};

/// `t(…)` read from a compiled screen with its recorded text, and `date(…)`, written as a `Table`
/// writes an instant; the other formats need `i18n`.
#[derive(Clone, Debug)]
#[cfg_attr(
    not(feature = "parse"),
    expect(
        dead_code,
        reason = "the position is written when a screen is compiled"
    )
)]
pub(crate) enum Call {
    Translate {
        key: String,
        arguments: Vec<Argument>,
        recorded: Option<Recorded>,
        position: Position,
    },
    Date {
        value: Expression,
        position: Position,
    },
}

impl Call {
    pub(crate) fn translate(
        key: String,
        arguments: Vec<Argument>,
        position: Position,
        recorded: Option<Recorded>,
    ) -> Self {
        Self::Translate {
            key,
            arguments,
            recorded,
            position,
        }
    }

    pub(crate) fn date(value: Expression, position: Position) -> Self {
        Self::Date { value, position }
    }

    #[cfg(any(feature = "analysis", feature = "gates"))]
    pub(crate) fn values(&self) -> impl Iterator<Item = &Expression> {
        let (arguments, value): (&[Argument], Option<&Expression>) = match self {
            Self::Translate { arguments, .. } => (arguments, None),
            Self::Date { value, .. } => (&[], Some(value)),
        };
        arguments
            .iter()
            .map(|argument| &argument.value)
            .chain(value)
    }

    #[cfg(feature = "parse")]
    pub(crate) fn position(&self) -> Position {
        match self {
            Self::Translate { position, .. } | Self::Date { position, .. } => *position,
        }
    }
}

#[cfg(feature = "parse")]
impl Call {
    pub(crate) fn arguments_mut(&mut self) -> impl Iterator<Item = &mut Expression> {
        let (arguments, value): (&mut [Argument], Option<&mut Expression>) = match self {
            Self::Translate { arguments, .. } => (arguments, None),
            Self::Date { value, .. } => (&mut [], Some(value)),
        };
        arguments
            .iter_mut()
            .map(|argument| &mut argument.value)
            .chain(value)
    }

    pub(crate) fn recorded_mut(&mut self) -> Option<(&str, &mut Option<Recorded>)> {
        match self {
            Self::Translate { key, recorded, .. } => Some((key, recorded)),
            Self::Date { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Argument {
    pub name: String,
    pub value: Expression,
}

impl Argument {
    #[cfg(feature = "client-compile")]
    pub(crate) fn plain_value(&self) -> &Expression {
        &self.value
    }
}

#[cfg(feature = "clients")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Texts;

#[cfg(feature = "clients")]
impl Texts {
    pub(crate) fn is_of_same_translations(self, other: Self) -> bool {
        self == other
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Language<'a> {
    lifetime: PhantomData<&'a ()>,
    #[cfg(feature = "gates")]
    pub(crate) gates: crate::rendering::GatesRef<'a>,
}

#[expect(
    clippy::unused_self,
    reason = "the same calls reach the translations under the `i18n` feature"
)]
impl Language<'_> {
    pub(crate) fn plain() -> Self {
        Self::default()
    }

    #[cfg(feature = "clients")]
    pub(crate) fn texts(self) -> Texts {
        Texts
    }

    pub(crate) fn text(self, _key: &str, fallback: &'static str) -> &'static str {
        fallback
    }

    #[cfg(feature = "listing")]
    pub(crate) fn field_error(
        self,
        _key: &str,
        _arguments: &[(&str, String)],
        fallback: &str,
    ) -> String {
        fallback.to_owned()
    }

    pub(crate) fn write_money(self, out: &mut String, amount: Money, _scratch: &mut String) {
        amount.write_major(out);
        out.push(' ');
        out.push_str(amount.currency().as_str());
    }
}

/// As the client writes it with ISO days; the offset only where the wall clock repeats.
pub(crate) fn write_instant(
    _language: Language<'_>,
    out: &mut String,
    instant: Instant,
    _scratch: &mut String,
) {
    let zone = DisplayZone::current();
    push_display(out, zone.wall_clock(instant).strftime("%Y-%m-%d %H:%M "));
    if let Some(offset) = zone.repeated_offset(instant) {
        out.push_str(&offset);
        out.push(' ');
    }
    push_escaped(out, zone.name());
}

pub(crate) fn label(name: &str, _position: Position) -> Expression {
    Expression::Authored(humanize(name))
}

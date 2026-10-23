//! A template's program as bytes: the header `sft` and the format version as
//! two little-endian bytes, then the ops. Lengths, counts and positions are
//! LEB128 integers, text is a length and UTF-8, an integer literal eight
//! little-endian bytes, and every op and expression starts with its tag.
//! Components are kept as written, by name and props, so the bytes do not
//! depend on how the catalog stores them.

mod read;
#[cfg(feature = "parse")]
mod write;

#[cfg(feature = "parse")]
use std::collections::HashMap;

use self::read::read;
#[cfg(feature = "parse")]
use self::write::write;
use crate::catalog::build_all;
#[cfg(feature = "parse")]
use crate::components::Components;
#[cfg(feature = "parse")]
use crate::parse::each_call;
use crate::syntax::{Op, Written};
use crate::{Template, TemplateError};

/// Raised whenever the bytes for the same template would change.
const FORMAT: u16 = 2;
const MAGIC: &[u8; 3] = b"sft";

mod tag {
    pub(super) const MARKUP: u8 = 0;
    pub(super) const TEXT: u8 = 1;
    pub(super) const URL: u8 = 2;
    pub(super) const IDENTIFIED: u8 = 3;
    pub(super) const COMPONENT: u8 = 4;
    pub(super) const IF: u8 = 5;
    pub(super) const EACH: u8 = 6;

    pub(super) const PATH: u8 = 0;
    pub(super) const QUOTED: u8 = 1;
    pub(super) const INTEGER: u8 = 2;
    pub(super) const AUTHORED: u8 = 3;
    pub(super) const INTERPOLATED: u8 = 4;
    pub(super) const TRANSLATE: u8 = 5;
    pub(super) const FORMAT_CALL: u8 = 6;
    pub(super) const LABEL: u8 = 7;
    pub(super) const GATE: u8 = 8;

    pub(super) const DATE_FORMAT: u8 = 2;
}

/// Reads what [`compile_template`] wrote, so a built app needs no parser.
pub fn load_template(name: &str, compiled: &[u8]) -> Result<Template, TemplateError> {
    let written = read(compiled).map_err(|message| TemplateError::unlocated(name, message))?;
    build(name, written)
}

#[cfg(feature = "parse")]
pub fn compile_template(name: &str, source: &str) -> Result<Vec<u8>, TemplateError> {
    compile_template_with(name, source, &Components::default())
}

/// The module components a screen writes are compiled into it.
#[cfg(feature = "parse")]
pub fn compile_template_with(
    name: &str,
    source: &str,
    components: &Components,
) -> Result<Vec<u8>, TemplateError> {
    let written = parse_written(name, source, components)?;
    let compiled = write(&written);
    checked(name, written)?;
    Ok(compiled)
}

/// Each `t(…)` call keeps its text in the source language of `translations`, so an app built
/// without `i18n` writes it.
#[cfg(all(feature = "parse", feature = "i18n"))]
pub fn compile_template_with_texts(
    name: &str,
    source: &str,
    components: &Components,
    translations: &crate::Translations,
) -> Result<Vec<u8>, TemplateError> {
    let mut written = parse_written(name, source, components)?;
    crate::translations::record(&mut written, translations.negotiate(None, None));
    let compiled = write(&written);
    checked(name, written)?;
    Ok(compiled)
}

#[cfg(feature = "parse")]
pub(crate) fn parse_template(
    name: &str,
    source: &str,
    components: &Components,
) -> Result<Template, TemplateError> {
    checked(name, parse_written(name, source, components)?)
}

/// As `Template::parse_with`, with each `t(…)` given the source-language text the screen's
/// `compiled` program recorded for its key: a development build rereads a screen this way, and a
/// build without `i18n` writes that text.
#[cfg(feature = "parse")]
pub fn parse_template_with_recorded(
    name: &str,
    source: &str,
    components: &Components,
    compiled: &[u8],
) -> Result<Template, TemplateError> {
    let mut program = read(compiled).map_err(|message| TemplateError::unlocated(name, message))?;
    let mut texts = HashMap::new();
    each_call(&mut program, &mut |call| {
        if let Some((key, recorded)) = call.recorded_mut()
            && let Some(text) = recorded.take()
        {
            texts.insert(key.to_owned(), text);
        }
    });
    let mut written = parse_written(name, source, components)?;
    each_call(&mut written, &mut |call| {
        if let Some((key, recorded)) = call.recorded_mut() {
            *recorded = texts.get(key).cloned();
        }
    });
    checked(name, written)
}

#[cfg(feature = "parse")]
fn parse_written(
    name: &str,
    source: &str,
    components: &Components,
) -> Result<Vec<Op<Written>>, TemplateError> {
    crate::parse::parse(source, components).map_err(|error| TemplateError::located(name, error))
}

#[cfg(feature = "parse")]
fn checked(name: &str, written: Vec<Op<Written>>) -> Result<Template, TemplateError> {
    let template = build(name, written)?;
    #[cfg(feature = "i18n")]
    crate::translations::key_uses(&template.ops)
        .map_err(|error| TemplateError::located(name, error))?;
    Ok(template)
}

pub(crate) fn build(name: &str, written: Vec<Op<Written>>) -> Result<Template, TemplateError> {
    let ops = build_all(written).map_err(|error| TemplateError::located(name, error))?;
    Ok(Template::new(name, ops))
}

#[cfg(all(test, feature = "parse", not(feature = "i18n")))]
mod tests {
    use serde_json::json;

    use super::{load_template, parse_template_with_recorded, write};
    use crate::Components;
    use crate::language::{Argument, Call};
    use crate::message::{Message, Recorded};
    use crate::syntax::{Expression, Op, Path, Position, Root};

    fn recorded_call(key: &str, recorded: Option<Recorded>) -> Op<crate::syntax::Written> {
        let count = Argument {
            name: "count".to_owned(),
            value: Expression::Path(Path {
                root: Root::Data,
                fields: vec!["count".to_owned()],
            }),
        };
        let position = Position { line: 1, column: 1 };
        let call = Call::translate(key.to_owned(), vec![count], position, recorded);
        Op::Text(Expression::Call(Box::new(call)))
    }

    #[test]
    fn a_build_without_i18n_writes_the_recorded_text_with_its_arguments() {
        let recorded = Recorded {
            other: Message::parse("{count} <orders>").unwrap(),
            one: Some(Message::parse("one order").unwrap()),
        };
        let compiled = write(&[recorded_call("orders.shown", Some(recorded))]);
        let loaded = load_template("orders", &compiled).unwrap();

        assert_eq!(
            loaded.render(&json!({"count": 2})).unwrap(),
            "2 &lt;orders&gt;"
        );
        assert_eq!(loaded.render(&json!({"count": 1})).unwrap(), "one order");
    }

    #[test]
    fn a_call_compiled_without_its_text_writes_its_key() {
        let compiled = write(&[recorded_call("orders.shown", None)]);
        let loaded = load_template("orders", &compiled).unwrap();

        assert_eq!(loaded.render(&json!({"count": 2})).unwrap(), "orders.shown");
    }

    #[test]
    fn a_screen_parsed_again_without_i18n_writes_the_texts_its_compiled_program_recorded() {
        let recorded = Recorded {
            other: Message::parse("{count} orders").unwrap(),
            one: None,
        };
        let compiled = write(&[recorded_call("orders.shown", Some(recorded))]);
        let source = "<p>{t(\"orders.shown\", count = data.count)}, {t(\"orders.new\")}</p>";

        let parsed =
            parse_template_with_recorded("orders", source, &Components::default(), &compiled)
                .unwrap();

        assert_eq!(
            parsed.render(&json!({"count": 2})).unwrap(),
            "<p>2 orders, orders.new</p>"
        );
    }
}

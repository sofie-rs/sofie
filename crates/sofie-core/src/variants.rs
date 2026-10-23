use std::borrow::Cow;
use std::mem::discriminant;

use serde::{Deserialize, Deserializer, Serializer};

use crate::{Cast, ColumnReader, ColumnSpec, Error, Result, SqlValue};

pub trait Variants: Copy + 'static {
    const TYPE: &'static str;
    /// Module-qualified, so two enums of one name keep their recorded moves apart.
    const PATH: &'static str = Self::TYPE;
    const VARIANTS: &'static [(&'static str, Self)];
    const TRANSITIONS: &'static [(Self, Self)] = &[];

    #[must_use]
    fn name(self) -> &'static str {
        Self::VARIANTS
            .iter()
            .find(|(_, variant)| same(*variant, self))
            .map(|(name, _)| *name)
            .expect("`#[derive(Enum)]` lists every variant")
    }

    fn to(self, next: Self) -> Result<Self> {
        let allowed = Self::TRANSITIONS
            .iter()
            .any(|(source, target)| same(*source, self) && same(*target, next));
        if allowed {
            crate::moves::record(Self::PATH, self.name(), next.name());
            return Ok(next);
        }
        Err(crate::moves::refused(
            snake_case(Self::TYPE),
            self.name(),
            next.name(),
        ))
    }
}

fn snake_case(name: &str) -> String {
    let mut snake = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.extend(character.to_lowercase());
    }
    snake
}

pub const COLUMNS: &[ColumnSpec] = String::COLUMNS;

fn same<E: Variants>(left: E, right: E) -> bool {
    discriminant(&left) == discriminant(&right)
}

#[must_use]
pub fn parse<E: Variants>(name: &str) -> Option<E> {
    E::VARIANTS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, variant)| *variant)
}

pub fn encode<E: Variants>(value: E, out: &mut Vec<SqlValue>) {
    out.push(SqlValue::Text(value.name().to_owned()));
}

pub fn decode<E: Variants>(row: &mut dyn ColumnReader) -> Result<E> {
    let stored = row.text()?;
    parse(stored)
        .ok_or_else(|| Error::internal_message(format!("`{stored}` is not a stored {}", E::TYPE)))
}

pub fn serialize<E: Variants, S: Serializer>(value: E, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.name())
}

pub fn deserialize<'de, E: Variants, D: Deserializer<'de>>(deserializer: D) -> Result<E, D::Error> {
    let given = Cow::<str>::deserialize(deserializer)?;
    parse(&given).ok_or_else(|| {
        let accepted: Vec<&str> = E::VARIANTS.iter().map(|(name, _)| *name).collect();
        serde::de::Error::custom(format!(
            "`{given}` is not a {}, expected one of {}",
            E::TYPE,
            accepted.join(", ")
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Light {
        Green,
        Red,
    }

    impl Variants for Light {
        const TYPE: &'static str = "Light";
        const VARIANTS: &'static [(&'static str, Self)] =
            &[("GREEN", Self::Green), ("RED", Self::Red)];
        const TRANSITIONS: &'static [(Self, Self)] = &[(Self::Green, Self::Red)];
    }

    #[test]
    fn names_and_parses_variants() {
        assert_eq!(Light::Red.name(), "RED");
        assert_eq!(parse::<Light>("GREEN"), Some(Light::Green));
        assert_eq!(parse::<Light>("BLUE"), None);
    }

    #[test]
    fn decodes_a_stored_name_and_refuses_an_unknown_one() {
        let values = [
            SqlValue::Text("RED".to_owned()),
            SqlValue::Text("BLUE".to_owned()),
        ];
        let mut row = crate::RowReader::new(&values);

        assert_eq!(decode::<Light>(&mut row).unwrap(), Light::Red);
        assert!(matches!(
            decode::<Light>(&mut row),
            Err(Error::Internal { .. })
        ));
    }

    #[test]
    fn allows_only_listed_transitions() {
        assert_eq!(Light::Green.to(Light::Red).unwrap(), Light::Red);
        let Err(Error::Invalid(refused)) = Light::Red.to(Light::Green) else {
            panic!("an unlisted move is a field error");
        };
        assert_eq!(
            refused,
            [
                crate::FieldError::new("light", "validation.move", "cannot move from RED to GREEN")
                    .with("from", "RED")
                    .with("to", "GREEN")
            ]
        );
    }
}

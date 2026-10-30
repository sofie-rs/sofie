use std::fmt::Display;

use serde_json::{Map, Number};

use crate::{Instant, Timestamp, Uuid, Value};

#[derive(Clone, Copy)]
pub enum Shown<'a> {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Number(&'a Number),
    Text(&'a str),
    /// Hex digits and hyphens only, so a page writes it without escaping or percent-encoding.
    Uuid(Uuid),
    /// Typed, so a screen tells it from text written like one.
    Instant(Instant),
    Formatted(&'a dyn Display),
    List(&'a dyn Items),
    Record(&'a dyn Fields),
}

pub trait Show {
    fn show(&self) -> Shown<'_>;
}

pub trait Items {
    fn item(&self, index: usize) -> Option<Shown<'_>>;
}

/// Named and ordered as the type's JSON; `#[derive(ReadModel)]` writes them for a read model.
pub trait Fields {
    fn name(&self, index: usize) -> Option<&str>;

    fn get(&self, name: &str) -> Option<Shown<'_>>;

    /// Overridden where `name(index)` walks from the start, so a walk over all fields stays linear.
    fn names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new((0..).map_while(|index| self.name(index)))
    }
}

impl<V: Value> Show for V
where
    V::Inner: Show,
{
    fn show(&self) -> Shown<'_> {
        self.inner().show()
    }
}

impl Show for serde_json::Value {
    fn show(&self) -> Shown<'_> {
        match self {
            Self::Null => Shown::Null,
            Self::Bool(flag) => Shown::Bool(*flag),
            Self::Number(number) => Shown::Number(number),
            Self::String(text) => Shown::Text(text),
            Self::Array(items) => Shown::List(items),
            Self::Object(fields) => Shown::Record(fields),
        }
    }
}

impl Fields for Map<String, serde_json::Value> {
    fn name(&self, index: usize) -> Option<&str> {
        self.keys().nth(index).map(String::as_str)
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        self.get(name).map(Show::show)
    }

    fn names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.keys().map(String::as_str))
    }
}

impl<T: Show> Items for Vec<T> {
    fn item(&self, index: usize) -> Option<Shown<'_>> {
        self.get(index).map(Show::show)
    }
}

impl<T: Show> Show for Vec<T> {
    fn show(&self) -> Shown<'_> {
        Shown::List(self)
    }
}

impl<T: Show> Show for Option<T> {
    fn show(&self) -> Shown<'_> {
        self.as_ref().map_or(Shown::Null, Show::show)
    }
}

macro_rules! show {
    ($($type:ty => |$value:ident| $shown:expr),* $(,)?) => {
        $(impl Show for $type { fn show(&self) -> Shown<'_> { let $value = self; $shown } })*
    };
}

show! {
    bool => |flag| Shown::Bool(*flag), String => |text| Shown::Text(text),
    i64 => |number| Shown::Integer(*number), i32 => |number| Shown::Integer(i64::from(*number)),
    u64 => |number| i64::try_from(*number).map_or(Shown::Formatted(number), Shown::Integer),
    f64 => |number| Shown::Float(*number), Uuid => |uuid| Shown::Uuid(*uuid),
    Timestamp => |timestamp| Shown::Instant(Instant::from(*timestamp)), jiff::civil::Date => |day| Shown::Formatted(day),
}

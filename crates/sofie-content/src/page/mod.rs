use std::convert::Infallible;
use std::fmt;

#[cfg(feature = "admin")]
mod factory;

#[cfg(feature = "admin")]
pub use factory::PageInput;

use sofie::prelude::*;
use sofie::serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie::values::{Control, Input, Present, Search, Url};

/// Its slug follows its title; a link to an earlier one is sent on to it.
#[derive(Aggregate)]
#[aggregate(behaviors(Slugged(from = title)))]
pub struct Page {
    meta: Meta,
    title: String,
    /// Shown by search engines and link previews.
    summary: String,
    body: Markdown,
    image: Option<Url>,
    published_at: Option<Instant>,
}

impl Page {
    /// Published again after a withdrawal, it carries the new time.
    pub fn publish(&mut self, now: Instant) {
        self.published_at.get_or_insert(now);
    }

    pub fn withdraw(&mut self) {
        self.published_at = None;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Markdown(String);

impl sofie::core::Value for Markdown {
    type Inner = String;

    fn inner(&self) -> &String {
        &self.0
    }

    fn try_from_inner(inner: String) -> Result<Self, impl fmt::Display> {
        Ok::<_, Infallible>(Self(inner))
    }
}

impl Serialize for Markdown {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Markdown {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl Present for Markdown {
    const INPUT: Input = Input::new(Control::Textarea);
    const SEARCH: Search = Search::Text;

    fn csv(&self) -> String {
        self.0.clone()
    }
}

use std::marker::PhantomData;

use crate::Rule;

/// `added` and `retire_after_days` are read from the source by `sofie check`, not at run time.
#[derive(Clone, Copy, Debug)]
pub struct Feature {
    name: &'static str,
    default: Option<fn() -> Rule>,
}

impl Feature {
    /// `name` is `<module>.<flag>`, unique in the app.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            default: None,
        }
    }

    /// Followed until a rule is stored for the flag; without one it is off.
    #[must_use]
    pub const fn default(self, rule: fn() -> Rule) -> Self {
        Self {
            default: Some(rule),
            ..self
        }
    }

    /// `date` is `YYYY-MM-DD`; `sofie check` reports the flag once it is older than
    /// `retire_after_days`, 90 unless set.
    #[must_use]
    pub const fn added(self, _date: &'static str) -> Self {
        self
    }

    #[must_use]
    pub const fn retire_after_days(self, _days: u32) -> Self {
        self
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    pub(crate) fn default_rule(self) -> Option<fn() -> Rule> {
        self.default
    }
}

/// A name is unique in the app, so it alone tells two features apart.
impl PartialEq for Feature {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Feature {}

pub struct Variants<V> {
    feature: Feature,
    variant: PhantomData<fn() -> V>,
}

impl<V> Clone for Variants<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Variants<V> {}

impl<V> Variants<V> {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            feature: Feature::new(name),
            variant: PhantomData,
        }
    }

    #[must_use]
    pub const fn added(self, _date: &'static str) -> Self {
        self
    }

    #[must_use]
    pub const fn retire_after_days(self, _days: u32) -> Self {
        self
    }

    #[must_use]
    pub const fn feature(self) -> Feature {
        self.feature
    }
}

use std::marker::PhantomData;

/// `added` and `retire_after_days` are read from the source by `sofie check`, not at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feature {
    name: &'static str,
}

impl Feature {
    /// `name` is `<module>.<flag>`, unique in the app.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self { name }
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
}

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

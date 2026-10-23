use std::sync::{Arc, Mutex, MutexGuard};

use sofie_core::unpoisoned;

use crate::flags::Snapshot;
use crate::{Feature, HeldSetting, SettingValue, Subject, Variants};

/// A flag nobody stored a rule for is off.
#[derive(Clone)]
pub struct Features {
    snapshot: Arc<Snapshot>,
    environment: Arc<str>,
    subject: Subject,
    evaluated: Arc<Mutex<Vec<(String, String)>>>,
    #[cfg(all(feature = "toolbar", debug_assertions))]
    overrides: Arc<[(String, String)]>,
}

impl Features {
    pub(crate) fn new(snapshot: Arc<Snapshot>, environment: Arc<str>, subject: Subject) -> Self {
        Self {
            snapshot,
            environment,
            subject,
            evaluated: Arc::default(),
            #[cfg(all(feature = "toolbar", debug_assertions))]
            overrides: Arc::new([]),
        }
    }

    /// Each answer is `on`, `off` or a variant's name, and wins over the stored rule.
    #[cfg(all(feature = "toolbar", debug_assertions))]
    #[must_use]
    pub fn overridden(self, overrides: Arc<[(String, String)]>) -> Self {
        Self { overrides, ..self }
    }

    #[must_use]
    pub fn enabled(&self, feature: Feature) -> bool {
        self.enabled_named(feature.name())
    }

    #[must_use]
    pub fn variant<V: sofie_core::Variants>(&self, variants: Variants<V>) -> Option<V> {
        let name = variants.feature().name();
        let chosen = match self.overriding(name) {
            Some("off") => None,
            Some("on") => self
                .snapshot
                .rules
                .get(name)
                .and_then(|rule| rule.variant(name, self.subject)),
            Some(variant) => Some(variant),
            None => self
                .snapshot
                .rules
                .get(name)
                .filter(|rule| rule.is_on(name, &self.environment, self.subject))
                .and_then(|rule| rule.variant(name, self.subject)),
        };
        self.record(name, chosen.unwrap_or("off"));
        let chosen = chosen?;
        let variant = sofie_core::variants::parse::<V>(chosen);
        if variant.is_none() {
            tracing::warn!(
                feature_flag.key = name,
                feature_flag.variant = chosen,
                "the flag's rule names a variant its enum does not have"
            );
        }
        variant
    }

    #[must_use]
    pub fn enabled_named(&self, name: &str) -> bool {
        let on = match self.overriding(name) {
            Some(answer) => answer != "off",
            None => self
                .snapshot
                .rules
                .get(name)
                .is_some_and(|rule| rule.is_on(name, &self.environment, self.subject)),
        };
        self.record(name, if on { "on" } else { "off" });
        on
    }

    /// `default` unless a value of its kind is stored.
    #[must_use]
    pub fn setting(&self, name: &str, default: SettingValue) -> SettingValue {
        match self.snapshot.settings.get(name) {
            Some(stored) if stored.same_kind(&default) => stored.clone(),
            _ => default,
        }
    }

    /// Sorted by name.
    #[must_use]
    pub fn settings(&self) -> Vec<HeldSetting> {
        self.snapshot.held_settings()
    }

    /// Changes whenever a rule or a setting does; equal on every instance holding the same ones.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.snapshot.version
    }

    #[must_use]
    pub fn evaluated(&self) -> String {
        let recorded = self.recorded();
        let pairs: Vec<String> = recorded
            .iter()
            .map(|(name, answer)| format!("{name}={answer}"))
            .collect();
        pairs.join(" ")
    }

    #[cfg_attr(
        not(all(feature = "toolbar", debug_assertions)),
        expect(
            clippy::unused_self,
            reason = "only a debug build's toolbar overrides a flag"
        )
    )]
    fn overriding(&self, name: &str) -> Option<&str> {
        #[cfg(all(feature = "toolbar", debug_assertions))]
        return self
            .overrides
            .iter()
            .find(|(overridden, _)| overridden == name)
            .map(|(_, answer)| answer.as_str());
        #[cfg(not(all(feature = "toolbar", debug_assertions)))]
        {
            let _ = name;
            None
        }
    }

    fn record(&self, name: &str, answer: &str) {
        let mut evaluated = self.recorded();
        if !evaluated.iter().any(|(recorded, _)| recorded == name) {
            evaluated.push((name.to_owned(), answer.to_owned()));
        }
    }

    fn recorded(&self) -> MutexGuard<'_, Vec<(String, String)>> {
        unpoisoned::lock(&self.evaluated)
    }
}

#[cfg(all(test, feature = "toolbar", debug_assertions))]
mod tests {
    use super::*;
    use crate::{Feature, Rule};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Layout {
        Classic,
        Compact,
    }

    impl sofie_core::Variants for Layout {
        const TYPE: &'static str = "Layout";
        const VARIANTS: &'static [(&'static str, Self)] =
            &[("classic", Self::Classic), ("compact", Self::Compact)];
    }

    #[test]
    fn an_override_wins_over_the_stored_rule() {
        let mut snapshot = Snapshot::default();
        snapshot
            .rules
            .insert("shop.beta".to_owned(), Rule::everyone());
        let subject = Subject {
            area: None,
            principal: None,
        };
        let overrides = [
            ("shop.beta", "off"),
            ("shop.split", "on"),
            ("shop.layout", "compact"),
        ]
        .map(|(name, answer)| (name.to_owned(), answer.to_owned()));

        let features = Features::new(Arc::new(snapshot), Arc::from("test"), subject)
            .overridden(Arc::new(overrides));

        assert!(!features.enabled(Feature::new("shop.beta")));
        assert!(features.enabled(Feature::new("shop.split")));
        assert_eq!(
            features.variant(Variants::<Layout>::new("shop.layout")),
            Some(Layout::Compact)
        );
        assert_eq!(
            features.evaluated(),
            "shop.beta=off shop.split=on shop.layout=compact"
        );
    }
}

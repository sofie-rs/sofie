use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

use sofie_core::{Error, Fnv1a64, Result, Timestamp, unpoisoned};
use sofie_db::{Channel, Db, Listening, Select};

use crate::store::{self, StoredFlag, StoredSetting};
use crate::{DeclaredSetting, Features, HeldSetting, Rule, SettingValue, Subject};

/// Fallback re-read should a change's wake-up be lost.
const EXPIRY: Duration = Duration::from_secs(30);
/// A burst of wake-ups (every committed event sends one) is read once.
const SETTLE: Duration = Duration::from_millis(200);

/// A request reads what is held and never waits for a refresh.
#[derive(Clone)]
pub struct Flags {
    shared: Arc<Shared>,
}

struct Shared {
    db: Db,
    environment: Arc<str>,
    held: RwLock<Arc<Snapshot>>,
    reloads: AtomicU64,
}

#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) rules: HashMap<String, Rule>,
    changed: HashMap<String, Timestamp>,
    pub(crate) settings: HashMap<String, SettingValue>,
    declared: Arc<[DeclaredSetting]>,
    pub(crate) version: u64,
    /// Which reload read it, so a slower reload that started earlier cannot replace it.
    reload: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub name: String,
    pub on: bool,
    pub since: Timestamp,
}

impl Flags {
    /// Listens before it reads, so a change committed once this returns always wakes it.
    pub async fn start(db: &Db, environment: &str) -> Result<Self> {
        let listening = Listening::start(db.clone(), Channel::Messages, EXPIRY).await;
        let flags = Self {
            shared: Arc::new(Shared {
                db: db.clone(),
                environment: Arc::from(environment),
                held: RwLock::default(),
                reloads: AtomicU64::new(0),
            }),
        };
        flags.shared.reload().await?;
        tokio::spawn(keep_fresh(Arc::downgrade(&flags.shared), listening));
        Ok(flags)
    }

    #[must_use]
    pub fn evaluate(&self, subject: Subject) -> Features {
        Features::new(self.held(), Arc::clone(&self.shared.environment), subject)
    }

    pub async fn set(&self, name: &str, rule: Rule) -> Result<()> {
        store::save_flag(&self.shared.db, name, rule).await?;
        self.shared.reload().await
    }

    /// Replaces those declared before.
    pub fn declare_settings(&self, declared: Vec<DeclaredSetting>) -> Result<()> {
        for (index, setting) in declared.iter().enumerate() {
            store::refuse_malformed_name("setting", &setting.name)?;
            if declared[..index]
                .iter()
                .any(|earlier| earlier.name == setting.name)
            {
                return Err(Error::internal_message(format!(
                    "setting `{}` is declared twice",
                    setting.name
                )));
            }
        }
        let mut held = unpoisoned::write(&self.shared.held);
        *held = Arc::new(Snapshot {
            declared: Arc::from(declared),
            ..Snapshot::clone(&held)
        });
        Ok(())
    }

    pub async fn change_setting(&self, name: &str, text: &str) -> Result<()> {
        let default = self
            .held()
            .declared
            .iter()
            .find(|declared| declared.name == name)
            .map(|declared| declared.default.clone())
            .ok_or_else(|| {
                Error::invalid("setting", format!("`{name}` is not a declared setting"))
            })?;
        store::save_setting(&self.shared.db, name, default.parse_like(text)?).await?;
        self.shared.reload().await
    }

    #[must_use]
    pub fn rules(&self) -> Vec<(String, Rule)> {
        let mut rules: Vec<(String, Rule)> = self
            .held()
            .rules
            .iter()
            .map(|(name, rule)| (name.clone(), rule.clone()))
            .collect();
        rules.sort_by(|left, right| left.0.cmp(&right.0));
        rules
    }

    #[must_use]
    pub fn settled(&self) -> Vec<Settled> {
        let held = self.held();
        let mut settled: Vec<Settled> = held
            .rules
            .iter()
            .filter(|(_, rule)| rule.is_settled())
            .filter_map(|(name, rule)| {
                Some(Settled {
                    name: name.clone(),
                    on: rule.on,
                    since: *held.changed.get(name)?,
                })
            })
            .collect();
        settled.sort_by(|left, right| left.name.cmp(&right.name));
        settled
    }

    fn held(&self) -> Arc<Snapshot> {
        Arc::clone(&unpoisoned::read(&self.shared.held))
    }
}

impl Shared {
    async fn reload(&self) -> Result<()> {
        let reload = self.reloads.fetch_add(1, Ordering::Relaxed) + 1;
        let read = self.db.read();
        let flags = StoredFlag::query().all(&read).await?;
        let settings = StoredSetting::query().all(&read).await?;
        install(&self.held, reload, |declared| {
            snapshot_of(flags, settings, declared)
        });
        Ok(())
    }
}

/// Under the lock, so declarations made while the tables were read are kept. A reload that
/// started later read the tables later, so an earlier one never replaces it.
fn install(
    held: &RwLock<Arc<Snapshot>>,
    reload: u64,
    build: impl FnOnce(Arc<[DeclaredSetting]>) -> Snapshot,
) {
    let mut held = unpoisoned::write(held);
    if held.reload > reload {
        return;
    }
    let snapshot = Snapshot {
        reload,
        ..build(Arc::clone(&held.declared))
    };
    *held = Arc::new(snapshot);
}

fn snapshot_of(
    flags: Vec<StoredFlag>,
    settings: Vec<StoredSetting>,
    declared: Arc<[DeclaredSetting]>,
) -> Snapshot {
    let mut snapshot = Snapshot {
        declared,
        ..Snapshot::default()
    };
    for flag in flags {
        let rule = match Rule::from_json(&flag.name, &flag.json) {
            Ok(rule) => rule,
            Err(error) => {
                tracing::warn!(flag = %flag.name, error = %error, "a stored flag is malformed; it stays off");
                continue;
            }
        };
        snapshot.changed.insert(flag.name.clone(), flag.changed_at);
        snapshot.rules.insert(flag.name, rule);
    }
    for setting in settings {
        let value = match SettingValue::from_json(&setting.name, &setting.json) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(setting = %setting.name, error = %error, "a stored setting is malformed; using its default");
                continue;
            }
        };
        let redeclared = snapshot
            .declared
            .iter()
            .any(|declared| declared.name == setting.name && !declared.default.same_kind(&value));
        if redeclared {
            tracing::warn!(
                setting = %setting.name,
                "the stored value is of another kind than the setting's declaration; using its default"
            );
        }
        snapshot.settings.insert(setting.name, value);
    }
    snapshot.version = version_of(&snapshot);
    snapshot
}

/// From the stored state alone, so every instance holding the same state agrees on it.
fn version_of(snapshot: &Snapshot) -> u64 {
    let mut entries: Vec<(&String, String)> = snapshot
        .rules
        .iter()
        .map(|(name, rule)| (name, rule.to_json()))
        .chain(
            snapshot
                .settings
                .iter()
                .map(|(name, value)| (name, value.to_json())),
        )
        .collect();
    entries.sort();
    entries
        .into_iter()
        .fold(Fnv1a64::new(), |hash, (name, stored)| {
            hash.write(name.as_bytes())
                .write(&[0])
                .write(stored.as_bytes())
                .write(&[0])
        })
        .finish()
}

impl Snapshot {
    pub(crate) fn held_settings(&self) -> Vec<HeldSetting> {
        let mut held: Vec<HeldSetting> = self
            .declared
            .iter()
            .map(|declared| HeldSetting {
                name: declared.name.clone(),
                value: self
                    .settings
                    .get(&declared.name)
                    .filter(|stored| stored.same_kind(&declared.default))
                    .unwrap_or(&declared.default)
                    .clone(),
                default: declared.default.clone(),
            })
            .collect();
        held.sort_by(|left, right| left.name.cmp(&right.name));
        held
    }
}

/// A failed read keeps what is held, so an unreachable database serves the last known state.
async fn keep_fresh(shared: Weak<Shared>, mut listening: Listening) {
    loop {
        listening.changed().await;
        tokio::time::sleep(SETTLE).await;
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if let Err(error) = shared.reload().await {
            tracing::warn!(error = %error, "reading the flags failed; keeping the last ones read");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_with(version: u64) -> impl FnOnce(Arc<[DeclaredSetting]>) -> Snapshot {
        move |declared| Snapshot {
            declared,
            version,
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_reload_that_started_earlier_does_not_replace_a_later_one() {
        let held = RwLock::default();

        install(&held, 2, read_with(20));
        install(&held, 1, read_with(10));

        assert_eq!(held.read().unwrap().version, 20);
    }
}

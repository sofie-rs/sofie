//! Created by sofie-postgres's `migrations/0004_flags.sql` and `0008_settings.sql`.

use std::marker::PhantomData;

use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, SqlType, SqlValue, Timestamp};
use sofie_db::{Aggregate, Column, Db, Field, Meta, Outcome, Plan, Select, Stamp, Step};

use crate::{Rule, SettingValue};

/// A flag's rule or a setting's value, kept as stored and read when held, so one malformed row
/// leaves out only its own entry.
pub(crate) struct Stored<T> {
    meta: Meta,
    pub(crate) name: String,
    pub(crate) json: String,
    pub(crate) changed_at: Timestamp,
    table: PhantomData<fn() -> T>,
}

pub(crate) trait Table: 'static {
    const NAME: &'static str;
    const FIELDS: &'static [Field];
}

pub(crate) enum FlagTable {}
pub(crate) enum SettingTable {}

pub(crate) type StoredFlag = Stored<FlagTable>;
pub(crate) type StoredSetting = Stored<SettingTable>;

const JSON_COLUMNS: &[ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];

impl Table for FlagTable {
    const NAME: &'static str = "flags";
    const FIELDS: &'static [Field] = &[
        Field::new("name", String::COLUMNS, false),
        Field::new("rule", JSON_COLUMNS, false),
        Field::new("changed_at", Timestamp::COLUMNS, false),
    ];
}

impl Table for SettingTable {
    const NAME: &'static str = "settings";
    const FIELDS: &'static [Field] = &[
        Field::new("name", String::COLUMNS, false),
        Field::new("value", JSON_COLUMNS, false),
        Field::new("changed_at", Timestamp::COLUMNS, false),
    ];
}

impl<T: Table> Aggregate for Stored<T> {
    const NAME: &'static str = T::NAME;
    const MODULE: &'static str = "sofie";
    const FIELDS: &'static [Field] = T::FIELDS;

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.name.encode(out);
        out.push(SqlValue::Text(self.json.clone()));
        self.changed_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            name: String::decode(row)?,
            json: row.text()?.to_owned(),
            changed_at: Timestamp::decode(row)?,
            table: PhantomData,
        })
    }
}

pub(crate) async fn save_flag(db: &Db, name: &str, rule: Rule) -> Result<()> {
    refuse_malformed_name("flag", name)?;
    rule.validate(name)?;
    save::<FlagTable>(db, name, rule.to_json()).await
}

pub(crate) async fn save_setting(db: &Db, name: &str, value: SettingValue) -> Result<()> {
    save::<SettingTable>(db, name, value.to_json()).await
}

async fn save<T: Table>(db: &Db, name: &str, json: String) -> Result<()> {
    let existing = Stored::<T>::query()
        .where_(Column::new("name").eq(name.to_owned()))
        .first(&db.read())
        .await?;
    let changed_at = db.clock().now().timestamp();
    let stored = match existing {
        Some(stored) => Stored {
            json,
            changed_at,
            ..stored
        },
        None => Stored {
            meta: Meta::new()?,
            name: name.to_owned(),
            json,
            changed_at,
            table: PhantomData,
        },
    };
    db.commit(&mut Changed(stored)).await
}

/// `field` names the input that carried `name`, so a form marks the right one.
pub(crate) fn refuse_malformed_name(field: &str, name: &str) -> Result<()> {
    let snake = |part: &str| {
        part.starts_with(|first: char| first.is_ascii_lowercase())
            && part.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
    };
    match name.split_once('.') {
        Some((module, rest)) if snake(module) && snake(rest) => Ok(()),
        _ => Err(Error::invalid(
            field,
            format!("`{name}` is not a {field} name such as `order.split_payments`"),
        )),
    }
}

/// Every instance reads the tables again on the wake-up this commits with.
struct Changed<A>(A);

impl<A: Aggregate> Outcome for Changed<A> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0.plan(plan)?;
        if let Some(notify) = plan.dialect().messages().notify {
            plan.push(Step::Script(notify.to_owned()));
        }
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }

    fn reply(self) -> Result<sofie_db::Reply> {
        self.0.reply()
    }
}

#[cfg(test)]
mod tests {
    use super::refuse_malformed_name;

    #[test]
    fn a_flag_name_is_module_dot_flag_in_snake_case() {
        assert!(refuse_malformed_name("flag", "order.split_payments").is_ok());
        for refused in [
            "split_payments",
            "Order.split",
            "order.",
            ".x",
            "order.split-payments",
        ] {
            assert!(refuse_malformed_name("flag", refused).is_err(), "{refused}");
        }
    }
}

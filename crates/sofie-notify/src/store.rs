//! Created by sofie-postgres's `migrations/0010_notify.sql`; each row is its principal's.

use sofie_core::{Cast, ColumnReader, ColumnSpec, Result, SqlType, SqlValue, Timestamp, Uuid};
use sofie_db::{Aggregate, Column, Field, Meta};
use sofie_values::TimeZone;

use crate::{Channel, Digest, Notice, TimeOfDay};

const TEXT: &[ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];

macro_rules! owned_by_principal {
    () => {
        fn meta(&self) -> &Meta {
            &self.meta
        }

        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }

        fn owner(&self) -> Option<Uuid> {
            Some(self.principal)
        }
    };
}

pub(crate) struct Settings {
    pub(crate) meta: Meta,
    pub(crate) principal: Uuid,
    pub(crate) zone: Option<TimeZone>,
    pub(crate) quiet_from: Option<TimeOfDay>,
    pub(crate) quiet_until: Option<TimeOfDay>,
    pub(crate) digest: Digest,
}

pub(crate) const SETTINGS_PRINCIPAL: Column<Settings, Uuid> = Column::new("principal");

impl Settings {
    pub(crate) fn new(principal: Uuid) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            principal,
            zone: None,
            quiet_from: None,
            quiet_until: None,
            digest: Digest::Off,
        })
    }
}

impl Aggregate for Settings {
    const NAME: &'static str = "notify_settings";
    const MODULE: &'static str = "sofie";
    const OWNER: Option<&'static str> = Some("principal");
    const FIELDS: &'static [Field] = &[
        Field::new("principal", Uuid::COLUMNS, false),
        Field::new("zone", TEXT, true),
        Field::new("quiet_from", TEXT, true),
        Field::new("quiet_until", TEXT, true),
        Field::new("digest", TEXT, false),
    ];
    const UNIQUE: &'static [&'static str] = &["principal"];

    owned_by_principal!();

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.principal.encode(out);
        self.zone.encode(out);
        text_of(self.quiet_from, out);
        text_of(self.quiet_until, out);
        out.push(SqlValue::Text(self.digest.to_stored()));
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            principal: Uuid::decode(row)?,
            zone: Option::decode(row)?,
            quiet_from: time_of(row)?,
            quiet_until: time_of(row)?,
            digest: Digest::from_stored(row.text()?)?,
        })
    }
}

fn text_of(time: Option<TimeOfDay>, out: &mut Vec<SqlValue>) {
    time.map(|time| time.to_string()).encode(out);
}

fn time_of(row: &mut dyn ColumnReader) -> Result<Option<TimeOfDay>> {
    Option::<String>::decode(row)?
        .map(|text| TimeOfDay::parse(&text))
        .transpose()
}

pub(crate) struct Choice {
    pub(crate) meta: Meta,
    pub(crate) principal: Uuid,
    pub(crate) topic: String,
    pub(crate) channel: Channel,
    pub(crate) enabled: bool,
}

pub(crate) const CHOICE_PRINCIPAL: Column<Choice, Uuid> = Column::new("principal");
pub(crate) const CHOICE_TOPIC: Column<Choice, String> = Column::new("topic");

impl Aggregate for Choice {
    const NAME: &'static str = "notify_choices";
    const MODULE: &'static str = "sofie";
    const OWNER: Option<&'static str> = Some("principal");
    const FIELDS: &'static [Field] = &[
        Field::new("principal", Uuid::COLUMNS, false),
        Field::new("topic", String::COLUMNS, false),
        Field::new("channel", TEXT, false),
        Field::new("enabled", bool::COLUMNS, false),
    ];

    owned_by_principal!();

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.principal.encode(out);
        self.topic.encode(out);
        out.push(SqlValue::Text(self.channel.as_str().to_owned()));
        self.enabled.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            principal: Uuid::decode(row)?,
            topic: String::decode(row)?,
            channel: Channel::parse(row.text()?)?,
            enabled: bool::decode(row)?,
        })
    }
}

pub(crate) struct InboxEntry {
    pub(crate) meta: Meta,
    pub(crate) principal: Uuid,
    pub(crate) topic: String,
    pub(crate) notice: Notice,
    pub(crate) created_at: Timestamp,
    pub(crate) read_at: Option<Timestamp>,
}

pub(crate) const INBOX_PRINCIPAL: Column<InboxEntry, Uuid> = Column::new("principal");
pub(crate) const INBOX_CREATED_AT: Column<InboxEntry, Timestamp> = Column::new("created_at");
pub(crate) const INBOX_READ_AT: Column<InboxEntry, Option<Timestamp>> = Column::new("read_at");

impl Aggregate for InboxEntry {
    const NAME: &'static str = "inbox";
    const MODULE: &'static str = "sofie";
    const OWNER: Option<&'static str> = Some("principal");
    const FIELDS: &'static [Field] = &[
        Field::new("principal", Uuid::COLUMNS, false),
        Field::new("topic", String::COLUMNS, false),
        Field::new("title", String::COLUMNS, false),
        Field::new("body", String::COLUMNS, false),
        Field::new("link", String::COLUMNS, true),
        Field::new("created_at", Timestamp::COLUMNS, false),
        Field::new("read_at", Timestamp::COLUMNS, true),
    ];

    owned_by_principal!();

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.principal.encode(out);
        self.topic.encode(out);
        encode_notice(&self.notice, out);
        self.created_at.encode(out);
        self.read_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            principal: Uuid::decode(row)?,
            topic: String::decode(row)?,
            notice: decode_notice(row)?,
            created_at: Timestamp::decode(row)?,
            read_at: Option::decode(row)?,
        })
    }
}

pub(crate) struct DigestItem {
    pub(crate) meta: Meta,
    pub(crate) principal: Uuid,
    pub(crate) email: String,
    pub(crate) notice: Notice,
    pub(crate) created_at: Timestamp,
    pub(crate) sent_at: Option<Timestamp>,
}

pub(crate) const DIGEST_PRINCIPAL: Column<DigestItem, Uuid> = Column::new("principal");
pub(crate) const DIGEST_SENT_AT: Column<DigestItem, Option<Timestamp>> = Column::new("sent_at");
pub(crate) const DIGEST_CREATED_AT: Column<DigestItem, Timestamp> = Column::new("created_at");

impl Aggregate for DigestItem {
    const NAME: &'static str = "notify_digest_items";
    const MODULE: &'static str = "sofie";
    const OWNER: Option<&'static str> = Some("principal");
    const FIELDS: &'static [Field] = &[
        Field::new("principal", Uuid::COLUMNS, false),
        Field::new("email", String::COLUMNS, false),
        Field::new("title", String::COLUMNS, false),
        Field::new("body", String::COLUMNS, false),
        Field::new("link", String::COLUMNS, true),
        Field::new("created_at", Timestamp::COLUMNS, false),
        Field::new("sent_at", Timestamp::COLUMNS, true),
    ];

    owned_by_principal!();

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.principal.encode(out);
        self.email.encode(out);
        encode_notice(&self.notice, out);
        self.created_at.encode(out);
        self.sent_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            principal: Uuid::decode(row)?,
            email: String::decode(row)?,
            notice: decode_notice(row)?,
            created_at: Timestamp::decode(row)?,
            sent_at: Option::decode(row)?,
        })
    }
}

fn encode_notice(notice: &Notice, out: &mut Vec<SqlValue>) {
    notice.title.encode(out);
    notice.body.encode(out);
    notice.link.encode(out);
}

fn decode_notice(row: &mut dyn ColumnReader) -> Result<Notice> {
    Ok(Notice {
        title: String::decode(row)?,
        body: String::decode(row)?,
        link: Option::decode(row)?,
    })
}

pub(crate) struct PushSubscription {
    pub(crate) meta: Meta,
    pub(crate) principal: Uuid,
    pub(crate) endpoint: String,
    pub(crate) p256dh: String,
    pub(crate) auth: String,
    pub(crate) created_at: Timestamp,
    pub(crate) gone_at: Option<Timestamp>,
}

pub(crate) const PUSH_PRINCIPAL: Column<PushSubscription, Uuid> = Column::new("principal");
pub(crate) const PUSH_ENDPOINT: Column<PushSubscription, String> = Column::new("endpoint");
pub(crate) const PUSH_GONE_AT: Column<PushSubscription, Option<Timestamp>> = Column::new("gone_at");

impl Aggregate for PushSubscription {
    const NAME: &'static str = "push_subscriptions";
    const MODULE: &'static str = "sofie";
    const OWNER: Option<&'static str> = Some("principal");
    const FIELDS: &'static [Field] = &[
        Field::new("principal", Uuid::COLUMNS, false),
        Field::new("endpoint", String::COLUMNS, false),
        Field::new("p256dh", String::COLUMNS, false),
        Field::new("auth", String::COLUMNS, false),
        Field::new("created_at", Timestamp::COLUMNS, false),
        Field::new("gone_at", Timestamp::COLUMNS, true),
    ];
    const UNIQUE: &'static [&'static str] = &["endpoint"];

    owned_by_principal!();

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.principal.encode(out);
        self.endpoint.encode(out);
        self.p256dh.encode(out);
        self.auth.encode(out);
        self.created_at.encode(out);
        self.gone_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            principal: Uuid::decode(row)?,
            endpoint: String::decode(row)?,
            p256dh: String::decode(row)?,
            auth: String::decode(row)?,
            created_at: Timestamp::decode(row)?,
            gone_at: Option::decode(row)?,
        })
    }
}

/// What a closing account takes along: sofie-identity's `account_data::Notifications`.
pub mod personal_data {
    use serde_json::{Value, json};
    use sofie_core::{Instant, Result, SqlValue, Uuid};
    use sofie_db::{
        Aggregate, LocalJobs, LocalScheduled, Outcome, Plan, Read, Select, Stamp, Step,
    };

    use super::{
        CHOICE_PRINCIPAL, Choice, DIGEST_PRINCIPAL, DIGEST_SENT_AT, DigestItem, INBOX_PRINCIPAL,
        InboxEntry, PUSH_GONE_AT, PUSH_PRINCIPAL, PushSubscription, SETTINGS_PRINCIPAL, Settings,
    };
    use crate::digest;

    /// What notify holds of a principal, for an account's data export.
    pub async fn exported(read: &Read, principal: Uuid) -> Result<Value> {
        let settings = Settings::query()
            .where_(SETTINGS_PRINCIPAL.eq(principal))
            .first(read)
            .await?;
        let choices = Choice::query()
            .where_(CHOICE_PRINCIPAL.eq(principal))
            .all(read)
            .await?;
        let inbox = InboxEntry::query()
            .where_(INBOX_PRINCIPAL.eq(principal))
            .all(read)
            .await?;
        let held = DigestItem::query()
            .where_(DIGEST_PRINCIPAL.eq(principal))
            .where_(DIGEST_SENT_AT.is_null())
            .all(read)
            .await?;
        let browsers = PushSubscription::query()
            .where_(PUSH_PRINCIPAL.eq(principal))
            .where_(PUSH_GONE_AT.is_null())
            .count(read)
            .await?;
        Ok(json!({
            "settings": settings.map(|settings| json!({
                "zone": settings.zone.as_ref().map(sofie_values::TimeZone::name),
                "quiet_from": settings.quiet_from,
                "quiet_until": settings.quiet_until,
                "digest": settings.digest.to_stored(),
            })),
            "channels": choices.iter().map(|choice| json!({
                "topic": choice.topic,
                "channel": choice.channel,
                "enabled": choice.enabled,
            })).collect::<Vec<_>>(),
            "inbox": inbox.iter().map(|entry| json!({
                "topic": entry.topic,
                "title": entry.notice.title,
                "body": entry.notice.body,
                "created_at": Instant::from(entry.created_at),
                "read_at": entry.read_at.map(Instant::from),
            })).collect::<Vec<_>>(),
            "held_for_digest": held.iter().map(|item| json!({
                "email": item.email,
                "title": item.notice.title,
                "created_at": Instant::from(item.created_at),
            })).collect::<Vec<_>>(),
            "subscribed_browsers": browsers,
        }))
    }

    /// Erases everything notify holds of a principal and stops its digest, for a closing account.
    pub async fn forgetting(jobs: &LocalJobs, principal: Uuid) -> Result<Forgotten> {
        Ok(Forgotten {
            principal,
            digest: jobs.cancel(&digest::key(principal)).await?,
        })
    }

    #[must_use = "nothing is erased unless the handler returns it"]
    pub struct Forgotten {
        principal: Uuid,
        digest: LocalScheduled,
    }

    impl Outcome for Forgotten {
        fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
            sofie_db::forget_principal(plan, self.principal);
            let tables = [
                (Settings::MODULE, Settings::NAME),
                (Choice::MODULE, Choice::NAME),
                (InboxEntry::MODULE, InboxEntry::NAME),
                (DigestItem::MODULE, DigestItem::NAME),
                (PushSubscription::MODULE, PushSubscription::NAME),
            ];
            for (module, name) in tables {
                let dialect = plan.dialect();
                let sql = format!(
                    "DELETE FROM {} WHERE principal = {}",
                    dialect.qualified(module, name),
                    dialect.placeholder(1),
                );
                plan.push(Step::statement(
                    sql,
                    vec![SqlValue::Uuid(self.principal)],
                    None,
                ));
            }
            self.digest.plan(plan)
        }

        fn committed(&mut self, stamp: Stamp) {
            self.digest.committed(stamp);
        }
    }
}

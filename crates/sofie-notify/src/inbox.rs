use serde::Serialize;
use sofie_core::{Clock, Error, Instant, Result, SqlValue, Timestamp, Uuid};
use sofie_db::{Aggregate, Context, Id, Outcome, Plan, Read, Resolve, Select, Stamp, Step};
use sofie_http::{Area, Principal, RequestContext};

use crate::store::{INBOX_CREATED_AT, INBOX_PRINCIPAL, INBOX_READ_AT, InboxEntry};

/// The request principal's inbox. A screen showing it with `.live(…)` is refreshed as
/// notifications arrive, since the worker's commits are told to open screens.
pub struct Inbox {
    principal: Uuid,
    read: Read,
    clock: Clock,
}

impl<A: Area, M: Send> Resolve<RequestContext<A, M>> for Inbox {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        let principal = Principal::<A>::resolve(context).await?;
        Ok(Self {
            principal: principal.id().uuid(),
            read: context.read(),
            clock: context.db().clock(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InboxItem {
    pub id: Uuid,
    pub topic: String,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    pub created_at: Instant,
    pub unread: bool,
}

impl Inbox {
    pub async fn latest(&self, limit: u64) -> Result<Vec<InboxItem>> {
        let entries = InboxEntry::query()
            .where_(INBOX_PRINCIPAL.eq(self.principal))
            .order_by(INBOX_CREATED_AT.desc())
            .limit(limit)
            .all(&self.read)
            .await?;
        Ok(entries.into_iter().map(item).collect())
    }

    pub async fn unread(&self) -> Result<u64> {
        InboxEntry::query()
            .where_(INBOX_PRINCIPAL.eq(self.principal))
            .where_(INBOX_READ_AT.is_null())
            .count(&self.read)
            .await
    }

    /// Another principal's notification is not found.
    pub async fn mark_read(&self, id: Uuid) -> Result<MarkedRead> {
        let entry = InboxEntry::query()
            .where_(InboxEntry::ID.eq(Id::from_uuid(id)))
            .where_(INBOX_PRINCIPAL.eq(self.principal))
            .first(&self.read)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(self.marked(vec![entry]))
    }

    /// One statement, so a notice read meanwhile conflicts nothing, whatever the backlog.
    pub fn mark_all_read(&self) -> MarkedRead {
        MarkedRead(Marking::All {
            principal: self.principal,
            now: self.clock.now().timestamp(),
        })
    }

    fn marked(&self, mut entries: Vec<InboxEntry>) -> MarkedRead {
        let now = self.clock.now().timestamp();
        for entry in &mut entries {
            entry.read_at.get_or_insert(now);
        }
        MarkedRead(Marking::Entries(entries))
    }
}

fn item(entry: InboxEntry) -> InboxItem {
    InboxItem {
        id: entry.meta.id(),
        unread: entry.read_at.is_none(),
        topic: entry.topic,
        title: entry.notice.title,
        body: entry.notice.body,
        link: entry.notice.link,
        created_at: Instant::from(entry.created_at),
    }
}

#[must_use = "nothing is marked read until the handler returns it"]
pub struct MarkedRead(Marking);

enum Marking {
    Entries(Vec<InboxEntry>),
    All { principal: Uuid, now: Timestamp },
}

impl Outcome for MarkedRead {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        match &self.0 {
            Marking::Entries(entries) => entries.plan(plan),
            Marking::All { principal, now } => {
                let dialect = plan.dialect();
                let sql = format!(
                    "UPDATE {} SET read_at = {}, version = version + 1 \
                     WHERE principal = {} AND read_at IS NULL",
                    dialect.qualified(InboxEntry::MODULE, InboxEntry::NAME),
                    dialect.placeholder(1),
                    dialect.placeholder(2),
                );
                let params = vec![SqlValue::Timestamp(*now), SqlValue::Uuid(*principal)];
                plan.push_as::<InboxEntry>(Step::statement(sql, params, None));
                plan.touched::<InboxEntry>(Some(*principal));
                Ok(())
            }
        }
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Marking::Entries(entries) = &mut self.0 {
            entries.committed(stamp);
        }
    }
}

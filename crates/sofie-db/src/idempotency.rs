use std::time::Duration;

use sofie_core::{Error, Result, SqlValue, Timestamp, Uuid};

use crate::{Context, Db, Outcome, Reach, Reply, Resolve, Step, query_values};

const KEPT_FOR: Duration = Duration::from_hours(24);
/// The claim commits with the work, so a claim left unanswered this long is work whose answer was
/// lost after it committed; it replays as empty rather than refusing every retry for a day.
const ANSWERED_WITHIN: Duration = Duration::from_mins(1);
const PENDING: &str = "A request with this key is still being answered; try again shortly.";
const REUSED: &str = "This key was already used for a different request.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// For a vendor call outside keyed work: build it once per call and send it on every retry.
    pub fn generate() -> Result<Self> {
        let id =
            Uuid::now_v7().map_err(|error| Error::internal("drawing an idempotency key", error))?;
        Ok(Self(id.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct Work {
    key: String,
    fingerprint: String,
}

impl Work {
    #[must_use]
    pub fn new(key: String, fingerprint: String) -> Self {
        Self { key, fingerprint }
    }

    #[must_use]
    pub fn key(&self) -> IdempotencyKey {
        IdempotencyKey(self.key.clone())
    }
}

pub trait KeyedWork: Context {
    fn work(&self) -> Option<&Work>;
}

impl KeyedWork for crate::worker::JobContext {
    fn work(&self) -> Option<&Work> {
        Some(&self.work)
    }
}

impl<C: KeyedWork> Resolve<C> for IdempotencyKey {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        let key = context.work().map(Work::key).ok_or_else(|| {
            Error::internal_message(
                "an IdempotencyKey is given only to a job or the handler of an idempotent route or a webhook",
            )
        });
        std::future::ready(key)
    }
}

pub struct Keyed<'a> {
    db: &'a Db,
    work: &'a Work,
}

impl<'a> Keyed<'a> {
    #[must_use]
    pub fn new(db: &'a Db, work: &'a Work) -> Self {
        Self { db, work }
    }

    pub async fn replay(&self) -> Result<Option<Reply>> {
        let dialect = self.db.driver().dialect();
        let sql = format!(
            "SELECT fingerprint, answered_at, reply, created_at FROM {} WHERE key = {} AND created_at > {}",
            table(self.db),
            dialect.placeholder(1),
            dialect.placeholder(2)
        );
        let params = [SqlValue::Text(self.work.key.clone()), kept_since(self.db)?];
        let rows = query_values(self.db.driver(), &sql, &params).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let lost_before = self.db.clock().now().checked_sub(ANSWERED_WITHIN)?;
        stored_answer(row, self.work, lost_before.timestamp())
    }

    /// When another request claimed the key first, its answer is returned and nothing written.
    pub async fn commit(&self, outcome: &mut dyn Outcome, reach: Reach) -> Result<Option<Reply>> {
        let claim = self.claim()?;
        let Err(error) = self.db.commit_with(outcome, vec![claim], reach).await else {
            return Ok(None);
        };
        match self.replay().await? {
            Some(reply) => Ok(Some(reply)),
            None => Err(error),
        }
    }

    pub async fn record(&self, reply: &Reply) -> Result<()> {
        let dialect = self.db.driver().dialect();
        let sql = format!(
            "UPDATE {} SET reply = {}, answered_at = {} WHERE key = {}",
            table(self.db),
            dialect.placeholder(1),
            dialect.placeholder(2),
            dialect.placeholder(3)
        );
        let reply = match reply {
            Reply::Empty => SqlValue::Null,
            answered => SqlValue::Json(answered.to_json()?),
        };
        let params = [
            reply,
            SqlValue::Timestamp(self.db.clock().now().timestamp()),
            SqlValue::Text(self.work.key.clone()),
        ];
        query_values(self.db.driver(), &sql, &params).await?;
        Ok(())
    }

    /// A live claim affects no row, which fails the commit as `Conflict`.
    fn claim(&self) -> Result<Step> {
        let dialect = self.db.driver().dialect();
        let placeholders: Vec<String> = (1..=4).map(|index| dialect.placeholder(index)).collect();
        let sql = format!(
            "INSERT INTO {} AS stored (key, fingerprint, created_at) VALUES ({}, {}, {}) \
             ON CONFLICT (key) DO UPDATE SET fingerprint = excluded.fingerprint, \
             created_at = excluded.created_at, answered_at = NULL, reply = NULL \
             WHERE stored.created_at <= {}",
            table(self.db),
            placeholders[0],
            placeholders[1],
            placeholders[2],
            placeholders[3]
        );
        Ok(Step::Statement {
            sql,
            params: vec![
                SqlValue::Text(self.work.key.clone()),
                SqlValue::Text(self.work.fingerprint.clone()),
                SqlValue::Timestamp(self.db.clock().now().timestamp()),
                kept_since(self.db)?,
            ],
            expect_rows: Some(crate::ExpectedRows::or_conflict(1)),
        })
    }
}

/// Run on a schedule, not by a request: a client must not choose when its request pays for it.
pub async fn sweep_expired_keys(db: &Db) -> Result<()> {
    let sql = format!(
        "DELETE FROM {} WHERE created_at <= {}",
        table(db),
        db.driver().dialect().placeholder(1)
    );
    query_values(db.driver(), &sql, &[kept_since(db)?]).await?;
    Ok(())
}

fn kept_since(db: &Db) -> Result<SqlValue> {
    let kept = db.clock().now().checked_sub(KEPT_FOR)?;
    Ok(SqlValue::Timestamp(kept.timestamp()))
}

fn table(db: &Db) -> String {
    db.driver().dialect().qualified("sofie", "idempotency")
}

fn stored_answer(row: Vec<SqlValue>, work: &Work, lost_before: Timestamp) -> Result<Option<Reply>> {
    match <[SqlValue; 4]>::try_from(row) {
        Ok([SqlValue::Text(fingerprint), ..]) if fingerprint != work.fingerprint => {
            Err(Error::Rule(REUSED.to_owned()))
        }
        Ok([_, SqlValue::Null, _, SqlValue::Timestamp(claimed)]) if claimed > lost_before => {
            Err(Error::Rule(PENDING.to_owned()))
        }
        Ok(
            [
                _,
                SqlValue::Null | SqlValue::Timestamp(_),
                SqlValue::Null,
                _,
            ],
        ) => Ok(Some(Reply::Empty)),
        Ok([_, SqlValue::Timestamp(_), SqlValue::Json(value), _]) => Ok(Some(Reply::Json(value))),
        row => Err(Error::internal_message(format!(
            "unexpected row in the stored answers: {:?}",
            row.map(|row| row.map(|value| value.kind()))
        ))),
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::Timestamp;

    use super::*;

    fn work() -> Work {
        Work::new("key".to_owned(), "fingerprint".to_owned())
    }

    #[test]
    fn a_generated_key_is_new_each_time() {
        let key = IdempotencyKey::generate().unwrap();

        assert_ne!(key, IdempotencyKey::generate().unwrap());
        assert_eq!(key.as_str().len(), 36);
    }

    fn row(fingerprint: &str, answered: bool, reply: SqlValue) -> Vec<SqlValue> {
        let answered_at = if answered {
            SqlValue::Timestamp(Timestamp::UNIX_EPOCH)
        } else {
            SqlValue::Null
        };
        let claimed = SqlValue::Timestamp(Timestamp::UNIX_EPOCH);
        vec![
            SqlValue::Text(fingerprint.to_owned()),
            answered_at,
            reply,
            claimed,
        ]
    }

    fn answer(row: Vec<SqlValue>) -> Result<Option<Reply>> {
        stored_answer(row, &work(), Timestamp::MIN)
    }

    #[test]
    fn an_answered_key_replays_its_reply() {
        let json = serde_json::json!({ "id": 1 });

        let empty = answer(row("fingerprint", true, SqlValue::Null)).unwrap();
        let answered = answer(row("fingerprint", true, SqlValue::Json(json.clone()))).unwrap();

        assert!(matches!(empty, Some(Reply::Empty)));
        assert!(matches!(answered, Some(Reply::Json(value)) if value == json));
    }

    #[test]
    fn a_pending_or_reused_key_is_refused() {
        let pending = answer(row("fingerprint", false, SqlValue::Null));
        let reused = answer(row("other", true, SqlValue::Null));

        assert!(matches!(pending, Err(Error::Rule(message)) if message == PENDING));
        assert!(matches!(reused, Err(Error::Rule(message)) if message == REUSED));
    }

    #[test]
    fn a_claim_whose_answer_was_lost_replays_as_empty_instead_of_pending() {
        let since_claimed = Timestamp::UNIX_EPOCH
            .checked_add(Duration::from_secs(1))
            .unwrap();

        let lost = stored_answer(
            row("fingerprint", false, SqlValue::Null),
            &work(),
            since_claimed,
        );

        assert!(matches!(lost, Ok(Some(Reply::Empty))), "{lost:?}");
    }
}

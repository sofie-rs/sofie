use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Timestamp, Uuid};

use crate::{Db, Step, query_decoded};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadMessage {
    pub id: Uuid,
    pub name: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub dead_at: Timestamp,
}

struct Stored {
    id: Uuid,
    name: String,
    attempts: i64,
    last_error: Option<String>,
    dead_at: Option<Timestamp>,
}

impl Stored {
    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            id: row.uuid()?,
            name: row.text()?.to_owned(),
            attempts: row.int()?,
            last_error: Option::decode(row)?,
            dead_at: Option::decode(row)?,
        })
    }

    fn dead(self) -> Option<DeadMessage> {
        Some(DeadMessage {
            dead_at: self.dead_at?,
            id: self.id,
            name: self.name,
            attempts: self.attempts,
            last_error: self.last_error,
        })
    }
}

const COLUMNS: &str = "id, name, attempts, last_error, dead_at";

pub struct DeadMessages {
    db: Db,
}

impl DeadMessages {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn newest(&self, limit: u64) -> Result<Vec<DeadMessage>> {
        let dialect = self.db.driver().dialect();
        let sql = format!(
            "SELECT {COLUMNS} FROM {} WHERE dead_at IS NOT NULL ORDER BY dead_at DESC LIMIT {}",
            dialect.qualified("sofie", "messages"),
            dialect.placeholder(1),
        );
        let limit = i64::try_from(limit)
            .map_err(|error| Error::internal("a limit too large to bind", error))?;
        let params = [SqlValue::Int(limit)];
        let stored = query_decoded(self.db.driver(), &sql, &params, Vec::new(), Stored::decode);
        Ok(stored.await?.into_iter().filter_map(Stored::dead).collect())
    }

    /// Logged at info as its audit, as no principal replays it.
    pub async fn replay(&self, id: Uuid) -> Result<DeadMessage> {
        let dead = self.dead_one(id).await?;
        let dialect = self.db.driver().dialect();
        let sql = format!(
            "UPDATE {} SET attempts = 0, dead_at = NULL, claimed_until = NULL, available_at = {} \
             WHERE id = {} AND attempts = {} AND dead_at IS NOT NULL",
            dialect.qualified("sofie", "messages"),
            dialect.placeholder(1),
            dialect.placeholder(2),
            dialect.placeholder(3),
        );
        let params = vec![
            SqlValue::Timestamp(self.db.clock().now().timestamp()),
            SqlValue::Uuid(id),
            SqlValue::Int(dead.attempts),
        ];
        match self
            .db
            .transaction(vec![Step::statement(sql, params, Some(1))])
            .await
        {
            Err(Error::Conflict) => Err(Error::Rule(format!(
                "message {id} was replayed or changed meanwhile; nothing was replayed"
            ))),
            outcome => outcome,
        }?;
        tracing::info!(
            message.id = %id,
            message.name = %dead.name,
            message.previous_attempts = dead.attempts,
            "replayed a dead message"
        );
        Ok(dead)
    }

    async fn dead_one(&self, id: Uuid) -> Result<DeadMessage> {
        let dialect = self.db.driver().dialect();
        let sql = format!(
            "SELECT {COLUMNS} FROM {} WHERE id = {}",
            dialect.qualified("sofie", "messages"),
            dialect.placeholder(1),
        );
        let params = [SqlValue::Uuid(id)];
        let found = query_decoded(self.db.driver(), &sql, &params, Vec::new(), Stored::decode)
            .await?
            .pop()
            .ok_or_else(|| Error::Rule(format!("there is no message {id}")))?;
        let name = found.name.clone();
        found.dead().ok_or_else(|| {
            Error::Rule(format!(
                "message {id} ({name}) is not dead: it waits or runs, and only a dead one is \
                 replayed"
            ))
        })
    }
}

//! Unlogged on Postgres, as a crash may lose a cache but nothing else. Created at start, not
//! by a migration, as the cache is opt-in; a later shape takes a new table name.

use std::sync::{Arc, Weak};
use std::time::Duration;

use sofie_core::{Error, Result, SqlType, SqlValue};
use sofie_db::{BoxFuture, Db, Engine, Step, query_decoded};

use crate::shared::{SharedStore, Stored, milliseconds};

const PRUNE_EVERY: Duration = Duration::from_secs(60);

struct CacheSql {
    create: &'static str,
    get: &'static str,
    set: &'static str,
    delete: &'static str,
    drop_tagged: &'static str,
    prune: &'static str,
    list: fn(&[String]) -> SqlValue,
    tags: fn(&[String]) -> SqlValue,
}

// The lock keeps instances starting together from racing on the catalog.
const POSTGRES: CacheSql = CacheSql {
    create: "SELECT pg_advisory_xact_lock(hashtext('sofie.cache'));
CREATE UNLOGGED TABLE IF NOT EXISTS sofie.cache (
    key text PRIMARY KEY,
    value text NOT NULL,
    tags text[] NOT NULL,
    expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS cache_tags ON sofie.cache USING gin (tags);
CREATE INDEX IF NOT EXISTS cache_expires_at ON sofie.cache (expires_at);",
    get: "SELECT value, array_to_string(tags, ' '),
    ceil(extract(epoch FROM expires_at - now()) * 1000)::bigint
FROM sofie.cache WHERE key = $1 AND expires_at > now()",
    set: "INSERT INTO sofie.cache (key, value, tags, expires_at)
VALUES ($1, $2, $3::text[], now() + $4::bigint * interval '1 millisecond')
ON CONFLICT (key) DO UPDATE
SET value = excluded.value, tags = excluded.tags, expires_at = excluded.expires_at",
    delete: "DELETE FROM sofie.cache WHERE key = ANY($1::text[])",
    drop_tagged: "DELETE FROM sofie.cache WHERE tags && $1::text[]",
    prune: "DELETE FROM sofie.cache WHERE expires_at <= now()",
    list: array,
    tags: array,
};

macro_rules! now {
    () => {
        "CAST(unixepoch('subsec') * 1000000 AS INTEGER)"
    };
}

// Tags kept as ` a b `, so each is found whole between spaces; lists bound as JSON.
const SQLITE: CacheSql = CacheSql {
    create: "CREATE TABLE IF NOT EXISTS sofie_cache (
    key text PRIMARY KEY,
    value text NOT NULL,
    tags text NOT NULL,
    expires_at integer NOT NULL
);
CREATE INDEX IF NOT EXISTS sofie_cache_expires_at ON sofie_cache (expires_at);",
    get: concat!(
        "SELECT value, trim(tags), (expires_at - ",
        now!(),
        " + 999) / 1000 FROM sofie_cache WHERE key = ?1 AND expires_at > ",
        now!()
    ),
    set: concat!(
        "INSERT INTO sofie_cache (key, value, tags, expires_at) VALUES (?1, ?2, ?3, ",
        now!(),
        " + ?4 * 1000)
ON CONFLICT (key) DO UPDATE
SET value = excluded.value, tags = excluded.tags, expires_at = excluded.expires_at"
    ),
    delete: "DELETE FROM sofie_cache WHERE key IN (SELECT value FROM json_each(?1))",
    drop_tagged: "DELETE FROM sofie_cache WHERE EXISTS (
    SELECT 1 FROM json_each(?1) WHERE instr(sofie_cache.tags, ' ' || json_each.value || ' ') > 0
)",
    prune: concat!("DELETE FROM sofie_cache WHERE expires_at <= ", now!()),
    list: json_list,
    tags: spaced,
};

fn array(values: &[String]) -> SqlValue {
    SqlValue::Array(values.iter().cloned().map(SqlValue::Text).collect())
}

fn json_list(values: &[String]) -> SqlValue {
    SqlValue::Text(serde_json::Value::from(values.to_vec()).to_string())
}

fn spaced(values: &[String]) -> SqlValue {
    SqlValue::Text(format!(" {} ", values.join(" ")))
}

pub(crate) struct Database {
    db: Db,
    sql: &'static CacheSql,
}

impl Database {
    pub(crate) async fn start(db: &Db) -> Result<Arc<Self>> {
        let driver = sofie_db::cache_driver(db);
        let sql = match driver.dialect().engine() {
            Engine::Postgres => &POSTGRES,
            Engine::Sqlite => &SQLITE,
        };
        driver
            .transaction(vec![Step::Script(sql.create.to_owned())])
            .await
            .map_err(|error| Error::internal("creating the cache table", error))?;
        let database = Arc::new(Self {
            db: db.clone(),
            sql,
        });
        tokio::spawn(prune(Arc::downgrade(&database)));
        Ok(database)
    }

    async fn write(&self, sql: &str, params: Vec<SqlValue>) -> Result<()> {
        let step = Step::statement(sql, params, None);
        sofie_db::cache_driver(&self.db)
            .transaction(vec![step])
            .await
    }
}

/// Reads skip expired rows, so pruning only bounds the table's size.
async fn prune(database: Weak<Database>) {
    loop {
        tokio::time::sleep(PRUNE_EVERY).await;
        let Some(database) = database.upgrade() else {
            return;
        };
        if let Err(error) = database.write(database.sql.prune, Vec::new()).await {
            tracing::warn!(error = %error, "pruning expired cache entries failed");
        }
    }
}

impl SharedStore for Database {
    fn name(&self) -> &'static str {
        "database"
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Stored>>> {
        Box::pin(async move {
            let params = [SqlValue::Text(key.to_owned())];
            let expects = vec![Some(SqlType::Text), Some(SqlType::Text), Some(SqlType::Int)];
            let rows = query_decoded(
                sofie_db::cache_driver(&self.db),
                self.sql.get,
                &params,
                expects,
                |row| {
                    let value = Arc::from(row.text()?);
                    let tags = row.text()?.split_whitespace().map(str::to_owned).collect();
                    let left = u64::try_from(row.int()?).unwrap_or(0);
                    Ok(Stored {
                        value,
                        tags,
                        expires_in: Duration::from_millis(left),
                    })
                },
            );
            Ok(rows.await?.into_iter().next())
        })
    }

    fn set<'a>(&'a self, key: &'a str, stored: &'a Stored) -> BoxFuture<'a, Result<()>> {
        let params = vec![
            SqlValue::Text(key.to_owned()),
            SqlValue::Text(stored.value.to_string()),
            (self.sql.tags)(&stored.tags),
            SqlValue::Int(milliseconds(stored.expires_in)),
        ];
        Box::pin(self.write(self.sql.set, params))
    }

    fn delete<'a>(&'a self, keys: &'a [String]) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.write(self.sql.delete, vec![(self.sql.list)(keys)]))
    }

    fn drop_tagged<'a>(&'a self, tags: &'a [String]) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.write(self.sql.drop_tagged, vec![(self.sql.list)(tags)]))
    }
}

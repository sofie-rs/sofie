use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use sofie_core::{Cast, Error, Fnv1a64, Result, SqlType, SqlValue};

use crate::{AggregateTable, Db, Engine, File, Step};

#[cfg(feature = "fresh")]
mod fresh;
#[cfg(feature = "migration-status")]
mod status;

#[cfg(feature = "migration-status")]
pub use status::{MigrationState, MigrationStatus, ModuleStatus};

#[derive(Clone, Copy, Debug)]
pub struct Migrations {
    pub(crate) module: &'static str,
    files: &'static [File],
    sqlite: &'static [File],
    pub(crate) aggregates: &'static [AggregateTable],
}

impl Migrations {
    #[must_use]
    pub const fn new(module: &'static str, files: &'static [File]) -> Self {
        Self {
            module,
            files,
            sqlite: &[],
            aggregates: &[],
        }
    }

    /// Run on SQLite instead of the files of the same name, where its SQL differs:
    /// SQLite names a module's tables after it, such as `order_order`.
    #[must_use]
    pub const fn sqlite(self, sqlite: &'static [File]) -> Self {
        Self { sqlite, ..self }
    }

    #[must_use]
    pub const fn aggregates(self, aggregates: &'static [AggregateTable]) -> Self {
        Self { aggregates, ..self }
    }

    fn in_name_order(&self, engine: Engine) -> Result<Vec<(&'static str, Cow<'static, str>)>> {
        let mut files = self.files.to_vec();
        if engine == Engine::Sqlite {
            files.retain(|file| !self.sqlite.iter().any(|own| own.name() == file.name()));
            files.extend_from_slice(self.sqlite);
        }
        files.sort_unstable_by(|first, second| name_order(first.name(), second.name()));
        files
            .into_iter()
            .map(|file| Ok((file.name(), normalized(file.text()?))))
            .collect()
    }
}

/// `10_x` runs after `9_y`; a name without a number runs after every numbered one.
pub(crate) fn name_order(first: &str, second: &str) -> Ordering {
    let key = |name: &str| {
        let digits = name
            .find(|character: char| !character.is_ascii_digit())
            .map_or(name, |end| &name[..end]);
        let number = digits.parse::<u64>().ok();
        (number.is_none(), number)
    };
    key(first).cmp(&key(second)).then(first.cmp(second))
}

/// CRLF line ends and a byte-order mark change neither the SQL nor its checksum.
fn normalized(sql: &'static str) -> Cow<'static, str> {
    let sql = sql.strip_prefix('\u{feff}').unwrap_or(sql);
    if sql.contains("\r\n") {
        return Cow::Owned(sql.replace("\r\n", "\n"));
    }
    Cow::Borrowed(sql)
}

/// Detects an applied file that was edited, not tampering.
fn checksum(text: &str) -> String {
    let hash = Fnv1a64::new().write(text.as_bytes()).finish();
    format!("{hash:016x}")
}

type Applied = HashMap<(String, String), String>;

#[derive(Clone, Copy)]
pub struct Schema<'db> {
    pub(crate) db: &'db Db,
}

impl Schema<'_> {
    /// An applied file that changed is refused; one named before files already
    /// applied, such as a module's role migration, is applied all the same.
    pub async fn migrate(self, modules: &[Migrations]) -> Result<()> {
        let db = self.db;
        let Some(locking) = db.driver().lock_migrations() else {
            let each_takes = db.driver().dialect().migrations_lock();
            return migrate_under(db, modules, Some(each_takes)).await;
        };
        let _lock = locking.await?;
        migrate_under(db, modules, None).await
    }
}

async fn migrate_under(db: &Db, modules: &[Migrations], lock: Option<&str>) -> Result<()> {
    let dialect = db.driver().dialect();
    let framework = dialect.framework_migrations();
    let setup = Step::Script(dialect.migrations_setup().to_owned());
    db.transaction(locked(lock, vec![setup])).await?;
    let applied = applied_migrations(db).await?;
    for migrations in std::iter::once(&framework).chain(modules) {
        for (name, sql) in migrations.in_name_order(dialect.engine())? {
            apply_unless_applied(db, &applied, migrations.module, name, &sql, lock).await?;
        }
    }
    Ok(())
}

fn locked(lock: Option<&str>, steps: Vec<Step>) -> Vec<Step> {
    lock.map(|sql| Step::Script(sql.to_owned()))
        .into_iter()
        .chain(steps)
        .collect()
}

async fn apply_unless_applied(
    db: &Db,
    applied: &Applied,
    module: &str,
    name: &str,
    sql: &str,
    lock: Option<&str>,
) -> Result<()> {
    let sum = checksum(sql);
    if is_applied(applied, module, name, &sum)? {
        return Ok(());
    }
    let Err(error) = apply(db, module, name, sql, &sum, lock).await else {
        return Ok(());
    };
    // Its record explains the failure if another process applied it meanwhile.
    let now = applied_migrations(db).await?;
    if is_applied(&now, module, name, &sum)? {
        return Ok(());
    }
    Err(error)
}

fn record_table(db: &Db) -> String {
    db.driver().dialect().qualified("sofie", "migrations")
}

async fn applied_migrations(db: &Db) -> Result<Applied> {
    let sql = format!("SELECT module, name, checksum FROM {}", record_table(db));
    let types = vec![Some(SqlType::Text); 3];
    let applied = crate::query_decoded(db.driver(), &sql, &[], types, |row| {
        let key = (String::decode(row)?, String::decode(row)?);
        Ok((key, String::decode(row)?))
    })
    .await?;
    Ok(applied.into_iter().collect())
}

async fn apply(
    db: &Db,
    module: &str,
    name: &str,
    sql: &str,
    sum: &str,
    lock: Option<&str>,
) -> Result<()> {
    let dialect = db.driver().dialect();
    let insert = format!(
        "INSERT INTO {} (module, name, checksum) VALUES ({}, {}, {})",
        record_table(db),
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    let params = [module, name, sum].map(|text| SqlValue::Text(text.to_owned()));
    let record = Step::statement(insert, params.to_vec(), None);
    let failed = |error| Error::internal(format!("migration {module}/{name}"), error);
    #[cfg(feature = "concurrent-indexes")]
    let record = match lock {
        None => match db.driver().migrate_alone(module, sql, record) {
            Ok(running) => return running.await.map_err(failed),
            Err(record) => record,
        },
        Some(_) => record,
    };
    let steps = vec![
        record,
        Step::Script(dialect.enter_schema(module)),
        Step::Script(sql.to_owned()),
    ];
    db.transaction(locked(lock, steps)).await.map_err(failed)
}

fn is_applied(applied: &Applied, module: &str, name: &str, sum: &str) -> Result<bool> {
    match applied.get(&(module.to_owned(), name.to_owned())) {
        None => Ok(false),
        Some(recorded) if recorded == sum => Ok(true),
        Some(_) => Err(Error::internal_message(format!(
            "migration {module}/{name} changed after it was applied"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_and_texts(
        files: Vec<(&'static str, Cow<'static, str>)>,
    ) -> Vec<(&'static str, String)> {
        files
            .into_iter()
            .map(|(name, sql)| (name, sql.into_owned()))
            .collect()
    }

    #[test]
    fn sqlite_runs_its_own_file_where_a_module_has_one() {
        const MIGRATIONS: Migrations = Migrations::new(
            "order",
            &[
                File::new("0001_a.sql", b"pg a"),
                File::new("0002_b.sql", b"pg b"),
            ],
        )
        .sqlite(&[File::new("sqlite/0001_a.sql", b"lite a")]);

        assert_eq!(
            names_and_texts(MIGRATIONS.in_name_order(Engine::Sqlite).unwrap()),
            [
                ("0001_a.sql", "lite a".to_owned()),
                ("0002_b.sql", "pg b".to_owned())
            ]
        );
        assert_eq!(
            names_and_texts(MIGRATIONS.in_name_order(Engine::Postgres).unwrap()),
            [
                ("0001_a.sql", "pg a".to_owned()),
                ("0002_b.sql", "pg b".to_owned())
            ]
        );
    }

    #[test]
    fn migrations_run_in_the_order_of_their_numbers() {
        const MIGRATIONS: Migrations = Migrations::new(
            "order",
            &[
                File::new("10_c.sql", b""),
                File::new("notes.sql", b""),
                File::new("9_b.sql", b""),
                File::new("1_a.sql", b""),
            ],
        );

        let names: Vec<&str> = MIGRATIONS
            .in_name_order(Engine::Postgres)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(names, ["1_a.sql", "9_b.sql", "10_c.sql", "notes.sql"]);
    }

    #[test]
    fn a_byte_order_mark_and_crlf_line_ends_change_neither_the_sql_nor_its_checksum() {
        const MIGRATIONS: Migrations = Migrations::new(
            "order",
            &[
                File::new("0001_lf.sql", b"CREATE TABLE a ();\nCREATE TABLE b ();\n"),
                File::new(
                    "0002_crlf.sql",
                    "\u{feff}CREATE TABLE a ();\r\nCREATE TABLE b ();\r\n".as_bytes(),
                ),
            ],
        );

        let files = MIGRATIONS.in_name_order(Engine::Postgres).unwrap();

        assert_eq!(files[1].1, files[0].1);
        assert_eq!(checksum(&files[1].1), checksum(&files[0].1));
    }

    #[test]
    fn checksum_changes_with_the_text_and_is_fnv_1a_of_it() {
        assert_eq!(checksum("create table a ()"), checksum("create table a ()"));
        assert_ne!(checksum("create table a ()"), checksum("create table b ()"));
        assert_eq!(checksum(""), "cbf29ce484222325");
    }
}

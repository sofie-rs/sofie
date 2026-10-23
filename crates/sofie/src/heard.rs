use sofie_core::Result;
use sofie_db::Db;

use crate::WorkerBuilder;

/// A process running no worker builds one only so its commits drop an event no worker hears;
/// the worker never claims, though building it may start what its jobs read, such as flags.
pub(crate) async fn learn(db: &Db, worker: WorkerBuilder<'_>) -> Result<()> {
    sofie_db::worker::publish_heard(&worker(db.clone()).await?);
    Ok(())
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use sofie_core::SqlValue;
    use sofie_db::worker::Worker;
    use sofie_db::{Meta, Outcome, Plan, query_values};

    use super::*;

    struct Restocked {}

    impl serde::Serialize for Restocked {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serde_json::Map::new().serialize(serializer)
        }
    }

    impl sofie_db::Input for Restocked {
        fn from_json(_: serde_json::Value) -> Result<Self> {
            Ok(Self {})
        }
    }

    impl sofie_db::Event for Restocked {
        const NAME: &'static str = "shop.Restocked";
    }

    struct Raising(Meta);

    impl Outcome for Raising {
        fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
            plan.raise(self.0.events());
            Ok(())
        }
    }

    /// A file, so a second connection reads what the first committed.
    struct File(std::path::PathBuf);

    impl File {
        fn new(name: &str) -> Self {
            let file = format!("sofie-heard-{name}-{}.db", std::process::id());
            Self(std::env::temp_dir().join(file))
        }

        fn open(&self) -> sofie_sqlite::Sqlite {
            sofie_sqlite::Sqlite::open(&self.0).unwrap()
        }

        async fn run(&self, script: &str) {
            let script = sofie_db::Step::Script(script.to_owned());
            sofie_db::Driver::transaction(&self.open(), vec![script])
                .await
                .unwrap();
        }

        async fn migrated(&self) -> Db {
            let db = Db::new(self.open());
            db.schema().migrate(&[]).await.unwrap();
            db
        }

        async fn queued_after_raising(&self, db: &Db) -> Vec<Vec<SqlValue>> {
            let mut meta = Meta::new().unwrap();
            meta.raise(Restocked {}).unwrap();
            db.commit(&mut Raising(meta)).await.unwrap();
            query_values(&self.open(), "SELECT count(*) FROM sofie_messages", &[])
                .await
                .unwrap()
        }
    }

    impl Drop for File {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    #[tokio::test]
    async fn a_web_process_s_commit_drops_an_event_no_worker_hears() {
        let file = File::new("deaf");
        let db = file.migrated().await;

        learn(&db, crate::worker_builder(Worker::new))
            .await
            .unwrap();

        assert_eq!(file.queued_after_raising(&db).await, [[SqlValue::Int(0)]]);
    }

    #[tokio::test]
    async fn a_web_process_s_commit_queues_an_event_a_worker_hears() {
        let file = File::new("hearing");
        let db = file.migrated().await;
        let hearing = |db| Worker::new(db).on::<Restocked, _>("shop.restock", restock);

        learn(&db, crate::worker_builder(hearing)).await.unwrap();

        assert_eq!(file.queued_after_raising(&db).await, [[SqlValue::Int(1)]]);
    }

    #[tokio::test]
    async fn a_listener_only_the_version_that_raised_an_event_has_waits_for_that_version() {
        let file = File::new("rolling");
        let db = file.migrated().await;
        let newer = |db| {
            Worker::new(db)
                .on::<Restocked, _>("shop.restock", restock)
                .on::<Restocked, _>("shop.announce_restock", restock)
        };
        learn(&db, crate::worker_builder(newer)).await.unwrap();
        file.queued_after_raising(&db).await;
        let older = Worker::new(db.clone()).on::<Restocked, _>("shop.restock", restock);

        assert_eq!(older.drain().await.unwrap(), 1);

        let waiting = query_values(&file.open(), "SELECT name FROM sofie_messages", &[])
            .await
            .unwrap();
        assert_eq!(
            waiting,
            [[SqlValue::Text("shop.announce_restock".to_owned())]]
        );
        assert_eq!(newer(db).drain().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_web_process_s_commit_writes_the_listener_jobs_and_no_event_row() {
        let file = File::new("no-event-row");
        let db = file.migrated().await;
        let record = "CREATE TABLE event_rows (name text NOT NULL);
            CREATE TRIGGER record_event_row AFTER INSERT ON sofie_messages WHEN NEW.kind = 'event'
            BEGIN INSERT INTO event_rows VALUES (NEW.name); END;";
        let hearing = |db| Worker::new(db).on::<Restocked, _>("shop.restock", restock);
        file.run(record).await;
        learn(&db, crate::worker_builder(hearing)).await.unwrap();

        assert_eq!(file.queued_after_raising(&db).await, [[SqlValue::Int(1)]]);

        let event_rows = query_values(&file.open(), "SELECT count(*) FROM event_rows", &[])
            .await
            .unwrap();
        assert_eq!(event_rows, [[SqlValue::Int(0)]]);
    }

    #[tokio::test]
    async fn a_queued_event_fans_out_to_every_listener_its_process_heard() {
        let file = File::new("queued-before");
        let db = file.migrated().await;
        file.run(
            "INSERT INTO sofie_messages (id, kind, name, payload, available_at) \
             VALUES ('0190a7d2-0000-7000-8000-000000000001', 'event', 'shop.Restocked', '{}', \
             CAST(unixepoch('subsec') * 1000000 AS INTEGER))",
        )
        .await;
        let first = Worker::new(db.clone()).on::<Restocked, _>("shop.restock", restock);
        let second = Worker::new(db.clone()).on::<Restocked, _>("shop.announce_restock", restock);
        sofie_db::worker::publish_heard(&second);

        assert_eq!(first.drain().await.unwrap(), 1);

        let waiting = query_values(&file.open(), "SELECT name FROM sofie_messages", &[])
            .await
            .unwrap();
        assert_eq!(
            waiting,
            [[SqlValue::Text("shop.announce_restock".to_owned())]]
        );
    }

    async fn restock() -> Result<()> {
        Ok(())
    }
}

use sofie::db::Db;
use sofie::sqlite::Sqlite;

#[tokio::test]
async fn its_tables_match_its_records_on_sqlite() {
    let db = Db::new(Sqlite::memory().unwrap());
    let migrations = [sofie_content::migrations()];

    db.schema().migrate(&migrations).await.unwrap();

    let mismatches = db.schema().check(&migrations).await.unwrap();
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

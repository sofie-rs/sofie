use sofie_core::{Error, Result, Uuid};
use sofie_db::{Db, Outcome, Plan, forget_principal, forgotten, unless_forgotten};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

struct Forgetting(Uuid);

impl Outcome for Forgetting {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        forget_principal(plan, self.0);
        Ok(())
    }
}

struct WritingFor(Uuid);

impl Outcome for WritingFor {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        unless_forgotten(plan, self.0);
        Ok(())
    }
}

#[tokio::test]
async fn a_forgotten_principal_is_read_as_such_and_refuses_a_write_planned_for_it() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let (closed, open) = (Uuid::from_u128(1), Uuid::from_u128(2));

    db.commit(&mut Forgetting(closed)).await.unwrap();
    db.commit(&mut Forgetting(closed)).await.unwrap();

    assert!(forgotten(&db.read(), closed).await.unwrap());
    assert!(!forgotten(&db.read(), open).await.unwrap());
    assert!(matches!(
        db.commit(&mut WritingFor(closed)).await,
        Err(Error::Conflict)
    ));
    db.commit(&mut WritingFor(open)).await.unwrap();
}

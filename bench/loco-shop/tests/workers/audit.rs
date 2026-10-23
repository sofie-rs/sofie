use loco_rs::{bgworker::BackgroundWorker, testing::prelude::*};
use loco_shop::{
    app::App,
    models::{audit_entries, orders, users},
    workers::audit::{Worker, WorkerArgs},
};
use sea_orm::EntityTrait;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn the_audit_worker_records_the_action() {
    let boot = boot_test::<App>().await.unwrap();
    let db = &boot.app_context.db;
    let params = users::RegisterParams {
        email: "ada@example.com".to_owned(),
        password: "secret123".to_owned(),
        name: "Ada".to_owned(),
    };
    let user = users::Model::create_with_password(db, &params)
        .await
        .unwrap();
    let order = orders::PlaceParams {
        email: "buyer@example.com".to_owned(),
        amount: "12.50".to_owned(),
        currency: "EUR".to_owned(),
    };
    let order = orders::Model::place(db, user.id, &order).await.unwrap();

    let args = WorkerArgs {
        order_id: order.id,
        action: "PLACED".to_owned(),
    };
    Worker::perform_later(&boot.app_context, args)
        .await
        .unwrap();

    let entries = audit_entries::Entity::find().all(db).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "PLACED");
}

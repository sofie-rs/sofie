use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "orders",
            &[
                ("id", ColType::PkAuto),
                ("email", ColType::String),
                ("status", ColType::String),
                ("total", ColType::BigInteger),
                ("currency", ColType::String),
                ("placed_at", ColType::TimestampWithTimeZone),
            ],
            &[("user", "")],
        )
        .await?;
        // The order list reads a user's newest orders; the expiry reads by status and age.
        add_index(m, "orders_user_placed_at", &["user_id", "placed_at"]).await?;
        add_index(m, "orders_status_placed_at", &["status", "placed_at"]).await
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "orders").await
    }
}

async fn add_index(m: &SchemaManager<'_>, name: &str, columns: &[&str]) -> Result<(), DbErr> {
    let mut index = Index::create();
    index.name(name).table(Alias::new("orders"));
    for column in columns {
        index.col(Alias::new(*column));
    }
    m.create_index(index).await
}

use loco_rs::prelude::*;

pub use super::_entities::audit_entries::{ActiveModel, Entity, Model};
pub type AuditEntries = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

impl Model {
    pub async fn record(db: &DatabaseConnection, order_id: i64, action: &str) -> ModelResult<Self> {
        let entry = ActiveModel {
            order_id: Set(order_id),
            action: Set(action.to_owned()),
            recorded_at: Set(chrono::Utc::now().into()),
            ..Default::default()
        };
        Ok(entry.insert(db).await?)
    }
}

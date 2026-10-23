use std::fmt;

use loco_rs::prelude::*;
use serde::Deserialize;
use validator::ValidationErrors;

pub use super::_entities::orders::{self, ActiveModel, Entity, Model};
pub type Orders = Entity;

pub const LIST_LIMIT: u64 = 50;

/// ISO 4217 currencies whose minor unit is not hundredths.
const ZERO_DIGIT_CURRENCIES: [&str; 16] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];
const THREE_DIGIT_CURRENCIES: [&str; 7] = ["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Paid,
    Shipped,
    Cancelled,
    Refunded,
    Expired,
}

impl OrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Paid => "PAID",
            Self::Shipped => "SHIPPED",
            Self::Cancelled => "CANCELLED",
            Self::Refunded => "REFUNDED",
            Self::Expired => "EXPIRED",
        }
    }

    fn parse(text: &str) -> ModelResult<Self> {
        [
            Self::Pending,
            Self::Paid,
            Self::Shipped,
            Self::Cancelled,
            Self::Refunded,
            Self::Expired,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
        .ok_or_else(|| ModelError::Message(format!("unknown order status {text}")))
    }

    pub const fn can_move_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Paid | Self::Cancelled | Self::Expired)
                | (Self::Paid, Self::Shipped | Self::Refunded)
        )
    }
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize, Validate)]
pub struct PlaceParams {
    #[validate(email(message = "must be an email address"))]
    pub email: String,
    pub amount: String,
    #[validate(length(equal = 3, message = "must be a 3-letter code"))]
    pub currency: String,
}

fn field_error(field: &'static str, code: &'static str, message: String) -> ModelError {
    let mut errors = ValidationErrors::new();
    errors.add(
        field,
        validator::ValidationError::new(code).with_message(message.into()),
    );
    ModelError::Validation(errors.into())
}

fn minor_digits(currency: &str) -> u32 {
    if ZERO_DIGIT_CURRENCIES.contains(&currency) {
        return 0;
    }
    if THREE_DIGIT_CURRENCIES.contains(&currency) {
        return 3;
    }
    2
}

/// Reads "149.90" EUR as 14990 minor units, exactly.
fn parse_minor(amount: &str, currency: &str) -> ModelResult<i64> {
    let digits = minor_digits(currency);
    let invalid = |message: String| field_error("amount", "amount", message);
    let decimal = Decimal::from_str_exact(amount.trim())
        .map_err(|_| invalid("must be a number such as 149.90".to_owned()))?;
    if decimal.scale() > digits {
        return Err(invalid(format!(
            "{currency} amounts have at most {digits} decimals"
        )));
    }
    let minor = decimal
        .checked_mul(Decimal::from(10_i64.pow(digits)))
        .and_then(|minor| i64::try_from(minor).ok())
        .ok_or_else(|| invalid("is out of range".to_owned()))?;
    if minor <= 0 {
        return Err(invalid("must be positive".to_owned()));
    }
    Ok(minor)
}

pub fn format_major(minor: i64, currency: &str) -> String {
    let digits = minor_digits(currency);
    Decimal::new(minor, digits).to_string()
}

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
    pub fn status(&self) -> ModelResult<OrderStatus> {
        OrderStatus::parse(&self.status)
    }

    pub async fn place(
        db: &DatabaseConnection,
        user_id: i64,
        params: &PlaceParams,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params).map_err(ModelError::Validation)?;
        if !params
            .currency
            .bytes()
            .all(|byte| byte.is_ascii_uppercase())
        {
            let message = "must be a 3-letter code".to_owned();
            return Err(field_error("currency", "currency", message));
        }
        let total = parse_minor(&params.amount, &params.currency)?;
        let order = ActiveModel {
            email: Set(params.email.trim().to_lowercase()),
            status: Set(OrderStatus::Pending.to_string()),
            total: Set(total),
            currency: Set(params.currency.clone()),
            placed_at: Set(chrono::Utc::now().into()),
            user_id: Set(user_id),
            ..Default::default()
        };
        Ok(order.insert(db).await?)
    }

    /// The user's newest orders.
    pub async fn list_for_user(db: &DatabaseConnection, user_id: i64) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .filter(orders::Column::UserId.eq(user_id))
            .order_by_desc(orders::Column::PlacedAt)
            .limit(LIST_LIMIT)
            .all(db)
            .await?)
    }

    /// Another user's order is not found, so it answers 404.
    pub async fn find_for_user(
        db: &DatabaseConnection,
        user_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .filter(orders::Column::UserId.eq(user_id))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Expires orders still pending after `placed_before`; returns how many.
    pub async fn expire_pending(
        db: &DatabaseConnection,
        placed_before: DateTimeWithTimeZone,
    ) -> ModelResult<u64> {
        let expired = Entity::update_many()
            .col_expr(
                orders::Column::Status,
                Expr::value(OrderStatus::Expired.as_str()),
            )
            .col_expr(orders::Column::UpdatedAt, Expr::current_timestamp())
            .filter(orders::Column::Status.eq(OrderStatus::Pending.as_str()))
            .filter(orders::Column::PlacedAt.lt(placed_before))
            .exec(db)
            .await?;
        Ok(expired.rows_affected)
    }
}

impl ActiveModel {
    pub async fn cancel(self, db: &DatabaseConnection) -> ModelResult<Model> {
        self.move_to(db, OrderStatus::Cancelled).await
    }

    async fn move_to(mut self, db: &DatabaseConnection, next: OrderStatus) -> ModelResult<Model> {
        let current = OrderStatus::parse(self.status.as_ref())?;
        if !current.can_move_to(next) {
            return Err(field_error(
                "status",
                "transition",
                format!("order cannot move from {current} to {next}"),
            ));
        }
        self.status = Set(next.to_string());
        Ok(self.update(db).await?)
    }
}

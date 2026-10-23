mod billing;
mod checkout;
mod entitlement;
mod entitlements;
mod events;
mod plan;
mod provider;
mod purchase;
mod seats;
mod setup;
mod status;
mod subscription;
mod usage;
mod webhook;

use sofie_db::{AggregateTable, Migrations};

pub use billing::{Billing, SeatsChangeAsked, Subscriber, TrialStarted, install};
pub use checkout::{Checkout, Link};
pub use entitlement::{Entitlement, Limit, Period, Quota, UNLIMITED};
pub use entitlements::Entitlements;
pub use events::{PurchaseCompleted, SubscriptionChanged};
pub use plan::{Plan, Plans, Product, SeatRule, Seats};
pub use provider::{ProviderKeys, Vendor};
pub use setup::{Setup, configure};
pub use status::{Standing, Status};
pub use usage::Consumed;
pub use webhook::WEBHOOK_PATH;

use sofie_db::worker::Worker;

const AGGREGATES: &[AggregateTable] = &[
    AggregateTable::of::<subscription::Subscription>(),
    AggregateTable::of::<purchase::Purchase>(),
    AggregateTable::of::<usage::Usage>(),
];

const MODULE_NAME: &str = "sofie_billing";

/// Its routes come from the `Billing` that `install` returns, as they need the app's plans.
pub const MODULE: sofie_http::Module = sofie_http::Module {
    migrations: Some(migrations()),
    jobs: worker,
    ..sofie_http::Module::new(MODULE_NAME)
};

#[must_use]
pub fn worker(worker: Worker) -> Worker {
    worker.on::<events::SeatsAsked, _>("sofie_billing.change_seats", seats::change_at_provider)
}

#[must_use]
pub const fn migrations() -> Migrations {
    Migrations::new(
        MODULE_NAME,
        sofie_db::embed!(
            "migrations",
            [
                "0001_create_billing.sql",
                "0002_provider_customer.sql",
                "0003_seats_asked.sql"
            ]
        ),
    )
    .sqlite(sofie_db::embed!(
        "migrations/sqlite",
        [
            "0001_create_billing.sql",
            "0002_provider_customer.sql",
            "0003_seats_asked.sql"
        ]
    ))
    .aggregates(AGGREGATES)
}

use sofie::db::Db;
use sofie::prelude::*;

mod wiring;

pub use wiring::{MODULES, worker};

pub async fn app(db: Db) -> Result<App> {
    Ok(wiring::app(db)?
        .area(
            "sofie_identity",
            sofie_identity::impersonation::stop_routes::<
                tracker_contract::area::Member,
                tracker_contract::area::Staff,
            >,
        )
        .area(
            "sofie_identity",
            sofie_identity::impersonation::routes::<
                tracker_contract::area::Staff,
                tracker_contract::area::Member,
            >,
        )
        .area(
            "sofie_tenancy",
            sofie_tenancy::organization_routes::<
                tracker_contract::area::Team,
                tracker_contract::area::Member,
            >,
        )
        .area(
            "sofie_tenancy",
            sofie_tenancy::member_routes::<
                tracker_contract::area::Member,
                tracker_contract::area::Team,
            >,
        ))
}

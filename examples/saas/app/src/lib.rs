use sofie::db::Db;
use sofie::flags::Flags;
use sofie::prelude::*;
use tracker_contract::area::{Member, Staff, Team};

mod wiring;

pub use wiring::{MODULES, worker};

pub async fn app(db: Db, flags: Flags) -> Result<App> {
    Ok(wiring::app(db, flags)?
        .area(
            "sofie_tenancy",
            sofie_tenancy::organization_routes::<Team, Member>,
        )
        .area(
            "sofie_tenancy",
            sofie_tenancy::member_routes::<Member, Team>,
        )
        .area(
            "sofie_identity",
            sofie_identity::impersonation::routes::<Staff, Member>,
        )
        .area(
            "sofie_identity",
            sofie_identity::impersonation::stop_routes::<Member, Staff>,
        )
        .area("sofie_content", sofie_content::admin_routes::<Staff>)
        .area("sofie_webhooks", sofie_webhooks::routes::<Team>))
}

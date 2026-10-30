//! Organizations as owners: an organization area's principal is the organization, so its
//! records are the organization's, while the account acting in it is read from the session.
//! Members hold a role each and join by an invitation mailed to their address; with
//! `billing`, members and pending invitations are held to the seats the plan allows.

#[cfg(feature = "account-data")]
mod account_data;
mod acting;
mod invitation;
mod members;
mod organization;
mod organizations;

use sofie::db::Migrations;

#[cfg(feature = "account-data")]
pub use account_data::OrganizationMemberships;
pub use acting::Acting;
pub use invitation::Invitation;
pub use members::{ROUTES as ORGANIZATION_ROUTES, routes as organization_routes};
pub use organization::{
    MemberJoined, MemberLeft, MemberRoleChanged, Organization, OrganizationArea,
    OrganizationDeleted, OrganizationId, OrganizationMember, OrganizationRole,
};
pub use organizations::{ROUTES as MEMBER_ROUTES, routes as member_routes};

const MODULE_NAME: &str = "sofie_tenancy";

/// Its routes are mounted in the app's own areas (`member_routes`, `organization_routes`).
pub const MODULE: sofie::http::Module = sofie::http::Module {
    migrations: Some(migrations()),
    #[cfg(feature = "billing")]
    jobs: worker,
    ..sofie::http::Module::new(MODULE_NAME)
};

#[cfg(feature = "billing")]
#[must_use]
pub fn worker(worker: sofie::worker::Worker) -> sofie::worker::Worker {
    worker.on::<sofie::billing::SubscriptionChanged, _>(
        "sofie_tenancy.cancel_deleted_plan",
        organization::cancel_deleted_plan,
    )
}

#[must_use]
pub const fn migrations() -> Migrations {
    Migrations::new(
        MODULE_NAME,
        sofie::db::embed!(
            "migrations",
            [
                "0000_module_role.sql",
                "0001_organizations.sql",
                "0002_organization_deleted.sql"
            ]
        ),
    )
    .sqlite(sofie::db::embed!(
        "migrations/sqlite",
        [
            "0000_module_role.sql",
            "0001_organizations.sql",
            "0002_organization_deleted.sql"
        ]
    ))
    .aggregates(AGGREGATES)
}

const AGGREGATES: &[sofie::db::AggregateTable] = &[
    sofie::db::AggregateTable::of::<Organization>(),
    sofie::db::AggregateTable::of::<OrganizationMember>(),
    sofie::db::AggregateTable::of::<Invitation>(),
];

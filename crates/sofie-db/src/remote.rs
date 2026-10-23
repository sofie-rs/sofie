//! Where a split app serves the apps it left, shared by its server and their clients.

use sofie_auth::Acting;

pub const PRINCIPAL_HEADER: &str = "sofie-principal";
pub const CONTRACTS_PATH: &str = "/_sofie/contracts/";
pub const LOOKUPS_PATH: &str = "/_sofie/lookups/";
pub const FEED_PATH: &str = "/_sofie/feed";

/// Both apps compile the DTO from its contract crate, so both name it alike.
#[must_use]
pub fn lookup_name<D>() -> &'static str {
    std::any::type_name::<D>()
}

tokio::task_local! {
    static ACTING: Option<Acting>;
}

pub async fn acting_as<F: Future>(acting: Option<Acting>, work: F) -> F::Output {
    ACTING.scope(acting, work).await
}

#[must_use]
pub fn acting() -> Option<Acting> {
    ACTING.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use sofie_core::Uuid;

    use super::*;

    #[tokio::test]
    async fn work_acts_for_the_principal_of_its_scope_only() {
        let customer = Acting {
            area: "customer".to_owned(),
            principal: Uuid::now_v7().unwrap(),
        };

        let inside = acting_as(Some(customer.clone()), async { acting() }).await;

        assert_eq!(inside, Some(customer));
        assert_eq!(acting(), None);
    }
}

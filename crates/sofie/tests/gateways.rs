#![cfg(all(feature = "client", feature = "fixtures"))]

use std::sync::Arc;

use sofie::client::{Adapter, NoSettings, Outgoing};
use sofie::core::Result;
use sofie::db::{Context, Db, Dep, Reach, Resolve, Writes};
use sofie_testing::FakeDriver;

/// Any root parses; the operation below opens no connection.
const ROOT: &[u8] = include_bytes!("../../sofie-client/tests/certificates/root.der");

#[sofie::gateway]
pub trait Payments: Send + Sync {
    async fn charge(&self, cents: i64) -> Result<String>;
}

pub struct Stripe {
    http: Outgoing,
}

impl Adapter for Stripe {
    const VENDOR: &'static str = "stripe";
    type Settings = NoSettings;

    fn new(http: Outgoing, _settings: NoSettings) -> Self {
        Self { http }
    }
}

#[sofie::gateway]
impl Payments for Stripe {
    async fn charge(&self, cents: i64) -> Result<String> {
        let authorized = self.http.operation("authorize", async move { Ok(cents) });
        Ok(format!("ch_{}", authorized.await?))
    }
}

struct TestContext(Db);

impl Context for TestContext {
    fn db(&self) -> &Db {
        &self.0
    }

    fn reach(&self) -> Reach {
        Reach::All
    }
}

impl Writes for TestContext {}

#[tokio::test]
async fn an_adapter_s_call_is_a_span_named_for_its_vendor_over_its_outgoing() {
    let settings = sofie_client::Vendor::default();
    let outgoing = sofie_client::testing::outgoing("stripe", settings, ROOT).unwrap();
    let stripe = Stripe::new(outgoing, NoSettings {});
    let telemetry = sofie::telemetry::capture();

    let charge = stripe.charge(250).await.unwrap();

    let spans = telemetry.spans();
    let named = |name: &str| spans.iter().find(|span| span.name == name).unwrap();
    assert_eq!(charge, "ch_250");
    assert_eq!(named("gateway").fields["otel.name"], "stripe.charge");
    assert_eq!(named("outgoing").fields["otel.name"], "stripe.authorize");
    assert_eq!(named("outgoing").parent, Some(named("gateway").id));
}

#[tokio::test]
async fn a_test_process_loads_no_adapter_so_a_test_provides_the_fake() {
    let db = Db::new(FakeDriver::default());
    sofie::fakes_from_tests();

    let loaded = sofie::client::load::<dyn Payments>(
        "payment",
        "payments",
        &[sofie::client::Choice::new(
            <Stripe as sofie::client::Adapter>::VENDOR,
            |keys| Ok(std::sync::Arc::new(sofie::client::adapter::<Stripe>(keys)?)),
        )],
    );
    sofie::client::provide::<dyn Payments>(&db);
    let unprovided = Dep::<dyn Payments>::resolve(&mut TestContext(db.clone())).await;
    let fake = Arc::new(FakePayments::default());
    sofie::db::provide::<dyn Payments>(&db, fake.clone());
    let payments = Dep::<dyn Payments>::resolve(&mut TestContext(db.clone()))
        .await
        .unwrap();

    assert!(loaded.is_ok());
    assert!(unprovided.is_err());
    assert!(payments.charge(100).await.is_ok());
    assert_eq!(fake.calls.made(), ["charge"]);
}

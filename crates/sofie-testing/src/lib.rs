mod app;
mod caller;
mod checks;
mod database;
mod display_zone;
mod fake;
mod fixture;
#[cfg(feature = "mail")]
mod mails;
mod messages;
mod request;
mod server;
mod start;
mod wiring;

pub use app::TestApp;
pub use caller::{TestCaller, TestPrincipal};
#[cfg(feature = "contracts")]
pub use checks::assert_lookup;
pub use checks::{
    assert_boundary, assert_moves, assert_schema, assert_store, assert_unique, assert_value,
    describe,
};
pub use database::{TestDatabase, finish_teardown};
pub use display_zone::set_display_zone;
pub use fake::FakeDriver;
#[cfg(feature = "contracts")]
pub use fake::fake;
pub use fixture::{AggregateFixture, FixtureOwner, fixture_input, fixture_value};
#[cfg(feature = "mail")]
pub use mails::sent_mails;
pub use messages::{assert_raised, drain_failing, raised};
pub use request::{TestRequest, TestResponse, TestSession};
pub use server::TestServer;
pub use sofie_macros::test_app;
pub use start::start;
#[cfg(feature = "flags")]
pub use start::start_with_flags;
pub use wiring::{AppWiring, WorkerWiring, run_test_app};

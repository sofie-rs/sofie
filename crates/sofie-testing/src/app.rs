use std::sync::Mutex;
use std::time::Duration;

use sofie_core::{SqlValue, TestClock};
use sofie_db::worker::Worker;
use sofie_db::{Db, Id};
use sofie_http::{Area, RoutePath};

use crate::caller::{TestCaller, TestPrincipal};
use crate::fixture::AggregateFixture;
use crate::request::{TestRequest, TestSession};

pub struct TestApp {
    pub(crate) db: Db,
    pub(crate) clock: TestClock,
    pub(crate) worker: Worker,
    pub(crate) postgres: sofie_postgres::Postgres,
    pub(crate) server: crate::TestServer,
    pub(crate) seed: Mutex<sofie_values::Seed>,
    #[cfg(feature = "flags")]
    pub(crate) flags: Option<sofie_flags::Flags>,
    #[cfg(feature = "mail")]
    pub(crate) outbox: sofie_mail::testing::Outbox,
    /// Declared last, so it is dropped after everything that uses it.
    pub(crate) _database: crate::TestDatabase,
}

impl TestApp {
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    #[must_use]
    pub fn clock(&self) -> &TestClock {
        &self.clock
    }

    #[cfg(feature = "flags")]
    #[must_use]
    pub fn flags(&self) -> &sofie_flags::Flags {
        self.flags
            .as_ref()
            .expect("the app was started with flags: `start_with_flags` or a wiring taking them")
    }

    /// Fails the test when a job fails or panics; `drain_failing` is for failures a test expects.
    pub async fn drain(&self) -> usize {
        crate::messages::drain(self).await
    }

    /// Stops at each moment a schedule, retry or delayed job comes due on the way, draining there.
    pub async fn advance(&self, by: Duration) -> usize {
        crate::messages::advance(self, by).await
    }

    pub async fn query(&self, sql: &str, params: &[SqlValue]) -> Vec<Vec<SqlValue>> {
        sofie_db::query_values(&self.postgres, sql, params)
            .await
            .unwrap_or_else(|error| panic!("running `{sql}`: {error}"))
    }

    /// `http://` and the address the app is served at, as a browser's `Origin` names it.
    #[must_use]
    pub fn origin(&self) -> &str {
        self.server.origin()
    }

    pub fn get(&self, path: &str) -> TestRequest {
        self.server.get(path)
    }

    pub fn post(&self, path: &str) -> TestRequest {
        self.server.post(path)
    }

    pub fn request(&self, route: &RoutePath) -> TestRequest {
        self.server.request(route.method().clone(), route.path())
    }

    pub fn as_<A: Area>(&self, principal: &TestPrincipal<A>) -> TestCaller<'_> {
        TestCaller::new(self, principal)
    }

    /// Whom a session the app issued signs in to area `A`, if anyone.
    #[must_use]
    pub fn principal_of<A: Area>(&self, session: &TestSession) -> Option<TestPrincipal<A>> {
        let claims = self.server.open(session, crate::messages::now(self))?;
        (claims.area() == A::NAME).then(|| TestPrincipal::new(Id::from_uuid(claims.principal())))
    }

    pub fn fixture<A: sofie_db::Create>(&self) -> AggregateFixture<'_, A> {
        AggregateFixture::new(self)
    }
}

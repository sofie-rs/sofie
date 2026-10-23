use std::fmt;
use std::marker::PhantomData;

use sofie_core::Uuid;
use sofie_db::Id;
use sofie_http::{Area, PrincipalRecord, RoutePath};

use crate::TestApp;
use crate::fixture::fixture_value;
use crate::messages::now;
use crate::request::{TestRequest, TestSession};

pub struct TestPrincipal<A: Area> {
    id: Uuid,
    area: PhantomData<fn() -> A>,
}

impl<A: Area> TestPrincipal<A> {
    #[must_use]
    pub fn new(id: Id<PrincipalRecord<A>>) -> Self {
        Self {
            id: id.uuid(),
            area: PhantomData,
        }
    }

    #[must_use]
    pub fn fresh(app: &TestApp) -> Self {
        Self::new(Id::from_uuid(fixture_value(app)))
    }

    #[must_use]
    pub fn id(&self) -> Id<PrincipalRecord<A>> {
        Id::from_uuid(self.id)
    }
}

impl<A: Area> fmt::Debug for TestPrincipal<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TestPrincipal<{}>({})", A::NAME, self.id)
    }
}

pub struct TestCaller<'app> {
    app: &'app TestApp,
    session: TestSession,
}

impl<'app> TestCaller<'app> {
    pub(crate) fn new<A: Area>(app: &'app TestApp, principal: &TestPrincipal<A>) -> Self {
        Self {
            app,
            session: app.server.seal(A::NAME, principal.id, now(app)),
        }
    }

    pub fn get(&self, path: &str) -> TestRequest {
        self.app.get(path).session(&self.session)
    }

    pub fn post(&self, path: &str) -> TestRequest {
        self.app.post(path).session(&self.session)
    }

    pub fn request(&self, route: &RoutePath) -> TestRequest {
        self.app.request(route).session(&self.session)
    }
}

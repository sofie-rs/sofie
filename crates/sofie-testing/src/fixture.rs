use std::future::IntoFuture;
use std::pin::Pin;
use std::sync::MutexGuard;

use serde::Serialize;
use serde_json::Value as Json;
use sofie_core::unpoisoned;
use sofie_db::{Aggregate, Create, Id, Input};
use sofie_values::{Fixture, InputFixture, Seed};

use crate::TestApp;

pub trait FixtureOwner {
    fn fixture_owner(app: &TestApp) -> Self;
}

impl FixtureOwner for () {
    fn fixture_owner(_app: &TestApp) -> Self {}
}

impl<T> FixtureOwner for Id<T> {
    fn fixture_owner(app: &TestApp) -> Self {
        Id::from_uuid(fixture_value(app))
    }
}

#[must_use = "a fixture is made only when awaited"]
pub struct AggregateFixture<'app, A: Create> {
    app: &'app TestApp,
    owner: Option<A::Owner>,
    fields: Vec<(String, Json)>,
}

impl<'app, A: Create> AggregateFixture<'app, A> {
    pub(crate) fn new(app: &'app TestApp) -> Self {
        Self {
            app,
            owner: None,
            fields: Vec::new(),
        }
    }

    pub fn owned_by(self, owner: A::Owner) -> Self {
        Self {
            owner: Some(owner),
            ..self
        }
    }

    pub fn with(mut self, field: &str, value: impl Serialize) -> Self {
        let value = serde_json::to_value(value).expect("a field's value serializes to JSON");
        self.fields.push((field.to_owned(), value));
        self
    }
}

impl<'app, A> IntoFuture for AggregateFixture<'app, A>
where
    A: Aggregate + Create<Owner: FixtureOwner + Send, Input: InputFixture>,
{
    type Output = A;
    type IntoFuture = Pin<Box<dyn Future<Output = A> + Send + 'app>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut input = fixture_input::<A::Input>(self.app);
        for (field, value) in self.fields {
            input[field] = value;
        }
        let owner = self
            .owner
            .unwrap_or_else(|| A::Owner::fixture_owner(self.app));
        let app = self.app;
        Box::pin(async move {
            let input = A::Input::from_json(input).unwrap_or_else(|error| {
                panic!("a fixture {} input is refused: {error:?}", A::NAME)
            });
            let mut record = A::create(input, owner).unwrap_or_else(|error| {
                panic!("the factory refuses a fixture {}: {error:?}", A::NAME)
            });
            app.db()
                .commit(&mut record)
                .await
                .unwrap_or_else(|error| panic!("committing a fixture {}: {error:?}", A::NAME));
            record
        })
    }
}

pub fn fixture_input<I: InputFixture>(app: &TestApp) -> Json {
    I::fixture_json(&mut seed(app))
}

pub fn fixture_value<T: Fixture>(app: &TestApp) -> T {
    T::fixture(&mut seed(app))
}

fn seed(app: &TestApp) -> MutexGuard<'_, Seed> {
    unpoisoned::lock(&app.seed)
}

use serde_json::{Value, json};
use sofie_core::Result;
use sofie_db::{Db, Migrations, Outcome, ReadOnly, Reply};
use sofie_http::{App, Module, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

struct Status(Value);

impl Outcome for Status {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Status {}

async fn status() -> Result<Status> {
    Ok(Status(json!("up")))
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/status", status);
}

const MODULES: &[Module] = &[
    Module {
        migrations: Some(Migrations::new("audit", &[])),
        ..Module::new("audit")
    },
    Module {
        areas: &[|app, module| app.area::<Web>(module, routes)],
        ..Module::new("order")
    },
];

#[tokio::test]
async fn each_module_s_areas_are_mounted() {
    let app = App::new(Db::new(FakeDriver::default())).modules(MODULES);

    let server = TestServer::start(app).await;

    server.get("/status").await.assert_status(200);
}

#[test]
fn the_migrations_are_those_of_the_modules_having_some_in_their_order() {
    let migrations = sofie_http::migrations(MODULES);

    assert_eq!(migrations.len(), 1);
    assert!(format!("{:?}", migrations[0]).contains("\"audit\""));
}

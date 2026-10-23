#![cfg(all(feature = "permissions", feature = "history"))]

use http::StatusCode;
use sofie_core::{Cast, ColumnReader, Result, SqlValue, Uuid};
use sofie_db::{
    Aggregate, Audited, BoxFuture, Db, Field, Id, Meta, Read, RecordsChanges, Step, Uses,
};
use sofie_http::{
    Access, Admitter, App, Area, AreaMembers, AreaPrincipal, NeedsPermission, Permission, Routes,
    Session, SignInRight, Web,
};
use sofie_testing::{FakeDriver, TestServer};

const WRITE_NOTES: Permission = Permission::new("notes.write");
const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);

struct StaffMember;

impl AreaPrincipal for StaffMember {
    const ACCESS: Access = Access::Permitted;
    type Record = Self;
}

struct Staff;

impl Area for Staff {
    const NAME: &'static str = "staff";
    const PREFIX: &'static str = "/staff";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = StaffMember;
}

struct EveryoneWrites;

impl AreaMembers for EveryoneWrites {
    fn grants<'a>(
        &'a self,
        _read: &'a Read,
        _area: &'static str,
        _principal: Uuid,
    ) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(std::future::ready(Ok(vec![WRITE_NOTES.name().to_owned()])))
    }
}

struct Note {
    meta: Meta,
    body: String,
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("body", String::COLUMNS, false)];
    const BEHAVIORS: &'static [&'static dyn sofie_db::Behavior] = &[&Audited];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.body.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            body: String::decode(row)?,
        })
    }
}

impl Uses<Audited> for Note {}

async fn add_note() -> Result<Note> {
    Ok(Note {
        meta: Meta::new()?,
        body: "Call the supplier".to_owned(),
    })
}

struct Tests;

impl Admitter for Tests {}

async fn sign_in_staff(session: Session) -> Result<()> {
    session.sign_in::<Staff>(SignInRight::of::<Tests>(), Id::from_uuid(MEMBER))?;
    Ok(())
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in_staff);
}

fn staff_routes(routes: &mut Routes<Staff>) {
    routes.post("/notes", add_note).permission(WRITE_NOTES);
}

#[tokio::test]
async fn a_change_made_in_a_permitted_area_names_its_principal() {
    let driver = FakeDriver::default();
    let app = App::new(Db::new(driver.clone()))
        .membership(EveryoneWrites)
        .area("notes", web_routes)
        .area("notes", staff_routes);
    let server = TestServer::start(app).await;
    let signed_in = server
        .post("/sign-in")
        .header("sec-fetch-site", "same-origin")
        .await;
    let cookie = signed_in.header("set-cookie");
    let cookie = cookie.split(';').next().unwrap().to_owned();
    drop(driver.take_transactions());

    let added = server
        .post("/staff/notes")
        .header("cookie", &cookie)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .await;

    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let inserted = driver
        .take_transactions()
        .into_iter()
        .flatten()
        .find_map(|step| match step {
            Step::Statement { sql, params, .. } if sql.contains("INSERT") => Some(params),
            _ => None,
        })
        .expect("the note was inserted");
    let actors = inserted
        .iter()
        .filter(|value| **value == SqlValue::Uuid(MEMBER))
        .count();
    assert_eq!(actors, 2, "created_by and updated_by: {inserted:?}");
}

struct Memo {
    meta: Meta,
}

impl Aggregate for Memo {
    const NAME: &'static str = "memo";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[];
    const BEHAVIORS: &'static [&'static dyn sofie_db::Behavior] = &[&RecordsChanges];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, _out: &mut Vec<SqlValue>) {}

    fn decode(meta: Meta, _row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self { meta })
    }
}

impl Uses<RecordsChanges> for Memo {}

async fn add_memo() -> Result<Memo> {
    Ok(Memo { meta: Meta::new()? })
}

fn memo_routes(routes: &mut Routes<Web>) {
    routes.post("/memos", add_memo).name("add_memo");
    routes.post("/memos/unnamed", add_memo);
}

async fn recorded_action(server: &TestServer, driver: &FakeDriver, path: &str) -> SqlValue {
    let added = server
        .post(path)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    driver
        .take_transactions()
        .into_iter()
        .flatten()
        .find_map(|step| match step {
            Step::Statement {
                sql, mut params, ..
            } if sql.contains("history") => Some(params.remove(7)),
            _ => None,
        })
        .expect("the change was recorded")
}

#[tokio::test]
async fn history_records_the_route_a_change_was_made_by_or_else_its_handler() {
    let driver = FakeDriver::default();
    let server =
        TestServer::start(App::new(Db::new(driver.clone())).area("notes", memo_routes)).await;

    let named = recorded_action(&server, &driver, "/memos").await;
    let unnamed = recorded_action(&server, &driver, "/memos/unnamed").await;

    assert_eq!(named, SqlValue::Text("notes.web.add_memo".to_owned()));
    assert_eq!(unnamed, SqlValue::Text("actor::add_memo".to_owned()));
}

#[cfg(feature = "jobs")]
async fn export_notes() -> Result<()> {
    Ok(())
}

#[cfg(feature = "jobs")]
#[tokio::test]
async fn a_job_queued_from_a_permitted_area_names_its_requester() {
    let driver = FakeDriver::default();
    let db = Db::new(driver.clone());
    sofie_db::name_job(&db, "notes.actions.export", export_notes);
    let app = App::new(db)
        .membership(EveryoneWrites)
        .area("notes", web_routes)
        .area("notes", |routes: &mut Routes<Staff>| {
            routes
                .queue("/notes/export", export_notes)
                .permission(WRITE_NOTES);
        });
    let server = TestServer::start(app).await;
    let signed_in = server
        .post("/sign-in")
        .header("sec-fetch-site", "same-origin")
        .await;
    let cookie = signed_in.header("set-cookie");
    let cookie = cookie.split(';').next().unwrap().to_owned();
    drop(driver.take_transactions());

    let queued = server
        .post("/staff/notes/export")
        .header("cookie", &cookie)
        .header("sec-fetch-site", "same-origin")
        .json(&serde_json::json!({}))
        .await;

    assert_eq!(queued.status, StatusCode::NO_CONTENT, "{}", queued.body);
    let job = driver
        .take_transactions()
        .into_iter()
        .flatten()
        .find_map(|step| match step {
            Step::Statement { params, .. }
                if params.contains(&SqlValue::Text("notes.actions.export".to_owned())) =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("the job was queued");
    assert_eq!(job[6], SqlValue::Uuid(MEMBER), "{job:?}");
}

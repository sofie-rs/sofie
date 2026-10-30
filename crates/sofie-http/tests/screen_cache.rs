#![cfg(feature = "screen-cache")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::StatusCode;
use serde_json::json;
use sofie_core::{Cast, ColumnReader, Result, SqlValue, Uuid};
use sofie_db::{Aggregate, Db, Field, Id, Meta, Outcome, Read, ReadOnly, Reply, Select};
use sofie_http::{Admitter, App, Area, Principal, Routes, Session, SignInRight, Web};
use sofie_testing::{FakeDriver, TestServer};

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const STRANGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);

struct Account;

impl sofie_http::AreaPrincipal for Account {
    const ACCESS: sofie_http::Access = sofie_http::Access::Owned;
    type Record = Self;
}

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

struct Note {
    meta: Meta,
    author: Id<Account>,
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("author", Uuid::COLUMNS, false)];
    const OWNER: Option<&'static str> = Some("author");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.author.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            author: Id::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.author.uuid())
    }
}

static LOADS: AtomicUsize = AtomicUsize::new(0);

struct Count(usize);

impl Outcome for Count {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({ "loads": self.0 })))
    }
}

impl ReadOnly for Count {}

async fn count_notes(read: Read) -> Result<Count> {
    Note::query().all(&read).await?;
    Ok(Count(LOADS.fetch_add(1, Ordering::SeqCst) + 1))
}

async fn write_mine(me: Principal<Members>) -> Result<Note> {
    Ok(Note {
        meta: Meta::new()?,
        author: me.id(),
    })
}

/// Signs in as the test's own admitter.
struct Tests;

impl Admitter for Tests {}

const RIGHT: SignInRight = SignInRight::of::<Tests>();

async fn sign_in_member(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(MEMBER))?;
    Ok(())
}

async fn sign_in_stranger(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(STRANGER))?;
    Ok(())
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in_member);
    routes.post("/sign-in/stranger", sign_in_stranger);
}

fn member_routes(routes: &mut Routes<Members>) {
    routes
        .screen("/notes", "notes", count_notes)
        .cached(Duration::from_secs(60));
    routes.post("/notes", write_mine);
}

async fn signed_in(server: &TestServer, path: &str) -> String {
    let answer = server
        .post(path)
        .header("sec-fetch-site", "same-origin")
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let cookie = answer.header("set-cookie");
    cookie.split(';').next().unwrap().to_owned()
}

async fn loads(server: &TestServer, cookie: &str, query: &str) -> String {
    let shown = server
        .get(&format!("/members/notes{query}"))
        .header("cookie", cookie)
        .await;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.body);
    shown.body
}

#[tokio::test]
async fn a_cached_screen_loads_again_only_for_another_reach_query_or_a_change() {
    let screens =
        sofie_http::screens_from_sources(&[("notes/members/notes", "<p>{data.loads}</p>")]);
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens.unwrap())
        .area("notes", web_routes)
        .area("notes", member_routes);
    let server = TestServer::start(app).await;
    let member = signed_in(&server, "/sign-in").await;
    let stranger = signed_in(&server, "/sign-in/stranger").await;

    assert!(loads(&server, &member, "").await.contains("<p>1</p>"));
    assert!(loads(&server, &member, "").await.contains("<p>1</p>"));
    assert!(loads(&server, &stranger, "").await.contains("<p>2</p>"));
    assert!(
        loads(&server, &member, "?page=2")
            .await
            .contains("<p>3</p>")
    );

    let written = server
        .post("/members/notes")
        .header("cookie", &member)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);

    assert!(loads(&server, &member, "").await.contains("<p>4</p>"));
    assert!(
        loads(&server, &stranger, "").await.contains("<p>2</p>"),
        "the member's own note does not reach the stranger's screen"
    );
}

#[cfg(feature = "listing")]
mod by_caller {
    use super::*;

    static CALLER_LOADS: AtomicUsize = AtomicUsize::new(0);

    async fn count_by_caller(read: Read) -> Result<Count> {
        Note::query().all(&read).await?;
        Ok(Count(CALLER_LOADS.fetch_add(1, Ordering::SeqCst) + 1))
    }

    fn caller_routes(routes: &mut Routes<Members>) {
        routes
            .screen("/counted", "counted", count_by_caller)
            .cached(Duration::from_secs(60));
    }

    #[tokio::test]
    async fn a_page_kept_for_a_browser_is_not_the_answer_to_a_json_caller() {
        let screens =
            sofie_http::screens_from_sources(&[("notes/members/counted", "<p>{data.loads}</p>")]);
        let app = App::new(Db::new(FakeDriver::default()))
            .screens(screens.unwrap())
            .area("notes", web_routes)
            .area("notes", caller_routes);
        let server = TestServer::start(app).await;
        let member = signed_in(&server, "/sign-in").await;

        let page = server
            .get("/members/counted")
            .header("cookie", &member)
            .await;
        let json = server
            .get("/members/counted")
            .header("cookie", &member)
            .accept_json()
            .await;

        assert!(page.body.contains("<p>1</p>"), "{}", page.body);
        assert_eq!(json.json()["loads"], 2, "{}", json.body);
    }
}

/// A hit reads no rows, so a tag taken from one would match any later content.
#[cfg(all(feature = "etag", not(feature = "development")))]
mod tagged {
    use super::*;

    static TAGGED_LOADS: AtomicUsize = AtomicUsize::new(0);

    async fn count_tagged(read: Read) -> Result<Count> {
        Note::query().all(&read).await?;
        Ok(Count(TAGGED_LOADS.fetch_add(1, Ordering::SeqCst) + 1))
    }

    fn tagged_routes(routes: &mut Routes<Members>) {
        routes
            .screen("/tagged", "tagged", count_tagged)
            .cached(Duration::from_secs(60));
        routes.post("/notes", write_mine);
    }

    #[tokio::test]
    async fn a_browser_holding_a_cached_screen_s_tag_sees_a_change() {
        let screens =
            sofie_http::screens_from_sources(&[("notes/members/tagged", "<p>{data.loads}</p>")]);
        let app = App::new(Db::new(FakeDriver::default()))
            .screens(screens.unwrap())
            .area("notes", web_routes)
            .area("notes", tagged_routes);
        let server = TestServer::start(app).await;
        let member = signed_in(&server, "/sign-in").await;
        let shown = |tag: Option<String>| {
            let mut request = server.get("/members/tagged").header("cookie", &member);
            if let Some(tag) = &tag {
                request = request.header("if-none-match", tag);
            }
            request
        };
        shown(None).await;
        let held = shown(None).await;
        let tag = held.header("etag").to_owned();

        let written = server
            .post("/members/notes")
            .header("cookie", &member)
            .header("sec-fetch-site", "same-origin")
            .accept_json()
            .await;
        assert_eq!(written.status, StatusCode::OK, "{}", written.body);
        shown(None).await;
        let again = shown(Some(tag.clone())).await;

        assert_eq!(again.status, StatusCode::OK, "held {tag}: {}", held.body);
        assert_ne!(again.body, held.body);
    }
}

#[cfg(feature = "permissions")]
mod permitted {
    use sofie_db::BoxFuture;
    use sofie_http::{Access, AreaMembers, AreaPrincipal, NeedsPermission, Permission};

    use super::*;

    const READ_NOTES: Permission = Permission::new("notes.read");

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

    struct EveryoneReads;

    impl AreaMembers for EveryoneReads {
        fn grants<'a>(
            &'a self,
            _read: &'a Read,
            _area: &'static str,
            _principal: Uuid,
        ) -> BoxFuture<'a, Result<Vec<String>>> {
            Box::pin(std::future::ready(Ok(vec![READ_NOTES.name().to_owned()])))
        }
    }

    struct Who(Uuid);

    impl Outcome for Who {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(json!({ "who": self.0.to_string() })))
        }
    }

    impl ReadOnly for Who {}

    async fn who_asks(me: Principal<Staff>) -> Result<Who> {
        Ok(Who(me.id().uuid()))
    }

    async fn sign_in_staff(session: Session) -> Result<()> {
        session.sign_in::<Staff>(RIGHT, sofie_db::Id::from_uuid(MEMBER))?;
        Ok(())
    }

    async fn sign_in_other_staff(session: Session) -> Result<()> {
        session.sign_in::<Staff>(RIGHT, sofie_db::Id::from_uuid(STRANGER))?;
        Ok(())
    }

    fn web_routes(routes: &mut Routes<Web>) {
        routes.post("/sign-in", sign_in_staff);
        routes.post("/sign-in/other", sign_in_other_staff);
    }

    fn staff_routes(routes: &mut Routes<Staff>) {
        routes
            .screen("/me", "me", who_asks)
            .cached(Duration::from_secs(60))
            .permission(READ_NOTES);
        routes.get("/grants", own_grants).permission(READ_NOTES);
    }

    /// As an owner check resolves, from the context rather than a handler argument.
    struct OwnGrants {
        reads_notes: bool,
        writes_notes: bool,
    }

    impl<M> sofie_db::Resolve<sofie_http::RequestContext<Staff, M>> for OwnGrants {
        fn resolve(
            context: &mut sofie_http::RequestContext<Staff, M>,
        ) -> impl Future<Output = Result<Self>> + Send {
            let permissions = context.permissions();
            std::future::ready(Ok(Self {
                reads_notes: permissions.can(READ_NOTES),
                writes_notes: permissions.can_named("notes.write"),
            }))
        }
    }

    impl Outcome for OwnGrants {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(json!([self.reads_notes, self.writes_notes])))
        }
    }

    impl ReadOnly for OwnGrants {}

    async fn own_grants(grants: OwnGrants) -> Result<OwnGrants> {
        Ok(grants)
    }

    #[tokio::test]
    async fn a_resolved_argument_reads_the_caller_s_own_grants() {
        let screens = sofie_http::screens_from_sources(&[("notes/staff/me", "<p>{data.who}</p>")]);
        let app = App::new(Db::new(FakeDriver::default()))
            .membership(EveryoneReads)
            .screens(screens.unwrap())
            .area("notes", web_routes)
            .area("notes", staff_routes);
        let server = TestServer::start(app).await;
        let cookie = signed_in(&server, "/sign-in").await;

        let answer = server
            .get("/staff/grants")
            .header("cookie", &cookie)
            .accept_json()
            .await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        assert_eq!(answer.json(), json!([true, false]));
    }

    #[tokio::test]
    async fn a_permitted_area_keeps_one_principal_s_cached_screen_from_another() {
        let screens = sofie_http::screens_from_sources(&[("notes/staff/me", "<p>{data.who}</p>")]);
        let app = App::new(Db::new(FakeDriver::default()))
            .membership(EveryoneReads)
            .screens(screens.unwrap())
            .area("notes", web_routes)
            .area("notes", staff_routes);
        let server = TestServer::start(app).await;
        let first = signed_in(&server, "/sign-in").await;
        let second = signed_in(&server, "/sign-in/other").await;

        let shown = |cookie: String| {
            let request = server.get("/staff/me").header("cookie", &cookie);
            async move {
                let answer = request.await;
                assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
                answer.body
            }
        };

        assert!(shown(first.clone()).await.contains(&MEMBER.to_string()));
        let other = shown(second).await;
        assert!(other.contains(&STRANGER.to_string()), "{other}");
        assert!(!other.contains(&MEMBER.to_string()), "{other}");
    }
}

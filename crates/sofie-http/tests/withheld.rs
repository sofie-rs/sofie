#[cfg(feature = "entitlements")]
mod entitled {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use sofie_core::{Result, Uuid};
    use sofie_db::{BoxFuture, Db, Id, Outcome, Read, ReadOnly, Reply};
    use sofie_http::{Admitter, App, Area, Entitled, Gates, Routes, Session, SignInRight, Web};
    use sofie_testing::{FakeDriver, TestServer};

    const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
    const UPGRADED: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);
    const PLAN_SCREEN: &str = r#"<h1>{data.name}</h1>
    {#if entitled("shop.export")}<p>{data.export_url}</p>{:else}<p>{data.upgrade_hint}</p>{/if}"#;

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

    struct Plans;

    impl Entitled for Plans {
        fn held<'a>(
            &'a self,
            _read: &'a Read,
            _area: &'static str,
            principal: Uuid,
        ) -> BoxFuture<'a, Result<Arc<[&'static str]>>> {
            let held: Arc<[&'static str]> = if principal == UPGRADED {
                Arc::from(["shop.export"])
            } else {
                Arc::from([])
            };
            Box::pin(std::future::ready(Ok(held)))
        }
    }

    struct Plan;

    impl Outcome for Plan {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(json!({
                "name": "Team",
                "export_url": "/exports/team.csv",
                "upgrade_hint": "Upgrade to export",
                "unbound": 1,
            })))
        }
    }

    impl ReadOnly for Plan {}

    async fn show_plan() -> Result<Plan> {
        Ok(Plan)
    }

    struct Tests;

    impl Admitter for Tests {}

    async fn sign_in_member(session: Session) -> Result<()> {
        session.sign_in::<Members>(SignInRight::of::<Tests>(), Id::from_uuid(MEMBER))?;
        Ok(())
    }

    async fn sign_in_upgraded(session: Session) -> Result<()> {
        session.sign_in::<Members>(SignInRight::of::<Tests>(), Id::from_uuid(UPGRADED))?;
        Ok(())
    }

    fn web_routes(routes: &mut Routes<Web>) {
        routes.post("/sign-in", sign_in_member);
        routes.post("/sign-in/upgraded", sign_in_upgraded);
    }

    fn member_routes(routes: &mut Routes<Members>) {
        routes.screen("/plan", "plan", show_plan);
    }

    async fn plan_server() -> TestServer {
        let screens = sofie_http::screens_from_sources(&[("shop/members/plan", PLAN_SCREEN)]);
        let app = App::new(Db::new(FakeDriver::default()))
            .screens(screens.unwrap())
            .gates(Gates::default().entitlements(Plans))
            .area("shop", web_routes)
            .area("shop", member_routes);
        TestServer::start(app).await
    }

    async fn plan_as(server: &TestServer, sign_in: &str, accept: &str) -> Value {
        let signed_in = server
            .post(sign_in)
            .header("sec-fetch-site", "same-origin")
            .await;
        let cookie = signed_in.header("set-cookie");
        let cookie = cookie.split(';').next().unwrap().to_owned();
        let plan = server
            .get("/members/plan")
            .header("cookie", &cookie)
            .header("accept", accept)
            .await;
        serde_json::from_str(&plan.body).unwrap()
    }

    #[tokio::test]
    async fn a_json_caller_failing_a_gate_gets_only_the_fields_it_is_shown() {
        let server = plan_server().await;

        let member = plan_as(&server, "/sign-in", "application/json").await;
        let upgraded = plan_as(&server, "/sign-in/upgraded", "application/json").await;

        assert_eq!(
            member,
            json!({
                "name": "Team",
                "export_url": null,
                "upgrade_hint": "Upgrade to export",
                "unbound": null,
            })
        );
        assert_eq!(upgraded["export_url"], "/exports/team.csv");
        assert_eq!(upgraded["upgrade_hint"], "Upgrade to export");
    }

    #[tokio::test]
    async fn a_client_envelope_failing_a_gate_gets_only_the_fields_it_is_shown() {
        let server = plan_server().await;
        let screen = "application/vnd.sofie.screen+json";

        let member = plan_as(&server, "/sign-in", screen).await;
        let upgraded = plan_as(&server, "/sign-in/upgraded", screen).await;

        assert_eq!(
            member["data"],
            json!({
                "name": "Team",
                "export_url": null,
                "upgrade_hint": "Upgrade to export",
                "unbound": null,
            })
        );
        assert_eq!(member["view"]["gates"]["entitlement"]["shop.export"], false);
        assert_eq!(upgraded["data"]["export_url"], "/exports/team.csv");
    }
}

#[cfg(feature = "permissions")]
mod permitted {
    use serde_json::{Value, json};
    use sofie_core::show::Fields;
    use sofie_core::{Instant, Show, Shown};
    use sofie_core::{Result, Uuid};
    use sofie_db::{BoxFuture, Db, Id, Outcome, Read, ReadOnly, Reply, ReplyData};
    use sofie_engine::ClientBundle;
    use sofie_http::{
        Access, Admitter, App, Area, AreaMembers, AreaPrincipal, NeedsPermission, Permission,
        Routes, Session, SignInRight, Web,
    };
    use sofie_testing::{FakeDriver, TestResponse, TestServer};

    const LIST: Permission = Permission::new("shop.order.*.list");
    const COST: &str = "shop.order.cost.read";
    const REFUND: &str = "shop.order.*.refund";
    const CLERK: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0003);
    const MANAGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0004);
    const ORDERS_SCREEN: &str = r#"<h1>{data.title}</h1>
{#each data.orders as order}<p>{order.id}</p>
{#if can("shop.order.cost.read")}<p>{order.cost}</p>{/if}{/each}
<Action post="/refunds/{data.refund_batch}" label="Refund" when={can("shop.order.*.refund")} />"#;
    const REPORT_SCREEN: &str = r#"<Table id="orders" of={data.orders} columns="id" />
{#if can("shop.order.cost.read")}<Table id="costed" of={data.orders} columns="id cost" />{/if}
{#if data.discount}<p>Discounted</p>{/if}
{#each data.charts as chart}<p>{chart.title}</p>
{#if can("shop.order.cost.read")}<p>{chart}</p>{/if}{/each}
{#if can("shop.order.cost.read")}<p>{data.discount}</p>{/if}"#;
    const PAID_SCREEN: &str = r#"<h1>{data.title}</h1>
{#if can("shop.order.cost.read")}<p>{data.paid_at}</p>{/if}"#;

    struct Member;

    impl AreaPrincipal for Member {
        const ACCESS: Access = Access::Permitted;
        type Record = Self;
    }

    struct Back;

    impl Area for Back {
        const NAME: &'static str = "back";
        const PREFIX: &'static str = "/back";
        const SIGN_IN: &'static str = "/sign-in";
        type Principal = Member;
    }

    struct Roles;

    impl AreaMembers for Roles {
        fn grants<'a>(
            &'a self,
            _read: &'a Read,
            _area: &'static str,
            principal: Uuid,
        ) -> BoxFuture<'a, Result<Vec<String>>> {
            let mut grants = vec![LIST.name().to_owned()];
            if principal == MANAGER {
                grants.extend([COST.to_owned(), REFUND.to_owned()]);
            }
            Box::pin(std::future::ready(Ok(grants)))
        }
    }

    struct Orders;

    impl Outcome for Orders {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(json!({
                "title": "Orders",
                "orders": [{ "id": 1, "cost": 40 }, { "id": 2, "cost": 75 }],
                "refund_batch": 7,
            })))
        }
    }

    impl ReadOnly for Orders {}

    struct Report;

    impl Outcome for Report {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(json!({
                "orders": { "items": [{ "id": 1, "cost": 40 }], "page": 1, "has_next": false },
                "discount": 5,
                "charts": [{ "title": "Sales", "points": [3, 4] }],
            })))
        }
    }

    impl ReadOnly for Report {}

    struct Paid {
        title: &'static str,
        paid_at: Instant,
    }

    impl Fields for Paid {
        fn name(&self, index: usize) -> Option<&str> {
            ["title", "paid_at"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "title" => Some(Shown::Text(self.title)),
                "paid_at" => Some(self.paid_at.show()),
                _ => None,
            }
        }
    }

    impl ReplyData for Paid {
        fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
            serde_json::to_writer(out, &self.to_json()?)
        }

        fn to_json(&self) -> serde_json::Result<Value> {
            Ok(json!({ "title": self.title, "paid_at": self.paid_at.to_string() }))
        }

        fn shown(&self) -> Option<Shown<'_>> {
            Some(Shown::Record(self))
        }
    }

    struct PaidOrder;

    impl Outcome for PaidOrder {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Data(Box::new(Paid {
                title: "Order 7",
                paid_at: Instant::parse("2026-10-11T22:30:00Z")?,
            })))
        }
    }

    impl ReadOnly for PaidOrder {}

    async fn show_paid() -> Result<PaidOrder> {
        Ok(PaidOrder)
    }

    async fn show_report() -> Result<Report> {
        Ok(Report)
    }

    #[cfg(feature = "chosen-screens")]
    async fn chosen_report() -> Result<sofie_http::Screen<Report>> {
        Ok(sofie_http::Screen::render("report", Report))
    }

    async fn reported() -> Result<()> {
        Ok(())
    }

    async fn list_orders() -> Result<Orders> {
        Ok(Orders)
    }

    struct Tests;

    impl Admitter for Tests {}

    async fn sign_in_clerk(session: Session) -> Result<()> {
        session.sign_in::<Back>(SignInRight::of::<Tests>(), Id::from_uuid(CLERK))?;
        Ok(())
    }

    async fn sign_in_manager(session: Session) -> Result<()> {
        session.sign_in::<Back>(SignInRight::of::<Tests>(), Id::from_uuid(MANAGER))?;
        Ok(())
    }

    fn web_routes(routes: &mut Routes<Web>) {
        routes.post("/sign-in", sign_in_clerk);
        routes.post("/sign-in/manager", sign_in_manager);
    }

    fn back_routes(routes: &mut Routes<Back>) {
        routes
            .screen("/orders", "orders", list_orders)
            .permission(LIST);
        routes
            .screen("/report", "report", show_report)
            .permission(LIST);
        routes.screen("/paid", "paid", show_paid).permission(LIST);
        routes
            .post("/report", reported)
            .permission(LIST)
            .then_screen("report", "orders");
        #[cfg(feature = "chosen-screens")]
        routes.get("/chosen", chosen_report).permission(LIST);
    }

    async fn orders_server() -> TestServer {
        let mut screens = sofie_http::screens_from_sources(&[
            ("shop/back/orders", ORDERS_SCREEN),
            ("shop/back/report", REPORT_SCREEN),
            ("shop/back/paid", PAID_SCREEN),
        ])
        .unwrap();
        screens.clients(vec![ClientBundle::new(
            "back",
            &["shop/back/orders", "shop/back/report", "shop/back/paid"],
            b"export {};",
            b"",
        )]);
        let app = App::new(Db::new(FakeDriver::default()))
            .screens(screens)
            .membership(Roles)
            .area("shop", web_routes)
            .area("shop", back_routes);
        TestServer::start(app).await
    }

    async fn signed_in(server: &TestServer, sign_in: &str) -> String {
        let signed_in = server
            .post(sign_in)
            .header("sec-fetch-site", "same-origin")
            .await;
        let cookie = signed_in.header("set-cookie");
        cookie.split(';').next().unwrap().to_owned()
    }

    async fn orders_as(server: &TestServer, sign_in: &str, accept: &str) -> TestResponse {
        get_as(server, sign_in, "/back/orders", accept).await
    }

    async fn get_as(server: &TestServer, sign_in: &str, path: &str, accept: &str) -> TestResponse {
        let cookie = signed_in(server, sign_in).await;
        server
            .get(path)
            .header("cookie", &cookie)
            .header("accept", accept)
            .await
    }

    async fn json_as(server: &TestServer, sign_in: &str) -> Value {
        let orders = orders_as(server, sign_in, "application/json").await;
        serde_json::from_str(&orders.body).unwrap()
    }

    async fn report_json_as(server: &TestServer, sign_in: &str, path: &str) -> Value {
        let report = get_as(server, sign_in, path, "application/json").await;
        serde_json::from_str(&report.body).unwrap()
    }

    fn report_for_a_clerk() -> Value {
        json!({
            "orders": { "items": [{ "id": 1, "cost": null }], "page": 1, "has_next": false },
            "discount": true,
            "charts": [{ "title": "Sales", "points": null }],
        })
    }

    fn envelope_in(page: &TestResponse) -> Value {
        let marker = "id=\"sofie-screen\">";
        let start = page.body.find(marker).unwrap() + marker.len();
        let end = start + page.body[start..].find("</script>").unwrap();
        serde_json::from_str(&page.body[start..end]).unwrap()
    }

    #[tokio::test]
    async fn a_field_of_each_item_behind_a_permission_reaches_only_its_holder() {
        let server = orders_server().await;

        let clerk = json_as(&server, "/sign-in").await;
        let manager = json_as(&server, "/sign-in/manager").await;

        assert_eq!(
            clerk,
            json!({
                "title": "Orders",
                "orders": [{ "id": 1, "cost": null }, { "id": 2, "cost": null }],
                "refund_batch": null,
            })
        );
        assert_eq!(manager["orders"][1]["cost"], 75);
    }

    #[tokio::test]
    async fn what_an_action_binds_is_withheld_where_its_when_fails() {
        let server = orders_server().await;

        let clerk = json_as(&server, "/sign-in").await;
        let manager = json_as(&server, "/sign-in/manager").await;

        assert_eq!(clerk["refund_batch"], Value::Null);
        assert_eq!(manager["refund_batch"], 7);
    }

    #[tokio::test]
    async fn the_envelope_a_client_shell_embeds_withholds_as_the_page_does() {
        let server = orders_server().await;

        let clerk = orders_as(&server, "/sign-in", "text/html").await;
        let manager = orders_as(&server, "/sign-in/manager", "text/html").await;

        let clerk = envelope_in(&clerk);
        assert_eq!(
            clerk["data"]["orders"],
            json!([{ "id": 1, "cost": null }, { "id": 2, "cost": null }])
        );
        assert_eq!(clerk["data"]["refund_batch"], Value::Null);
        assert_eq!(clerk["view"]["gates"]["permission"][COST], false);
        assert_eq!(envelope_in(&manager)["data"]["orders"][0]["cost"], 40);
    }
    #[tokio::test]
    async fn a_column_shown_only_behind_a_permission_reaches_only_its_holder() {
        let server = orders_server().await;

        let clerk = report_json_as(&server, "/sign-in", "/back/report").await;
        let manager = report_json_as(&server, "/sign-in/manager", "/back/report").await;

        assert_eq!(clerk["orders"], report_for_a_clerk()["orders"]);
        assert_eq!(manager["orders"]["items"][0]["cost"], 40);
    }

    #[tokio::test]
    async fn an_item_read_whole_behind_a_permission_sends_only_what_is_shown_outside() {
        let server = orders_server().await;

        let clerk = report_json_as(&server, "/sign-in", "/back/report").await;
        let manager = report_json_as(&server, "/sign-in/manager", "/back/report").await;

        assert_eq!(clerk["charts"], report_for_a_clerk()["charts"]);
        assert_eq!(manager["charts"][0]["points"], json!([3, 4]));
    }

    #[tokio::test]
    async fn a_value_only_tested_outside_a_permission_is_sent_as_whether_it_holds_anything() {
        let server = orders_server().await;

        let clerk = report_json_as(&server, "/sign-in", "/back/report").await;
        let manager = report_json_as(&server, "/sign-in/manager", "/back/report").await;

        assert_eq!(clerk["discount"], true);
        assert_eq!(manager["discount"], 5);
    }

    #[cfg(feature = "chosen-screens")]
    #[tokio::test]
    async fn a_screen_a_handler_chooses_withholds_as_its_route_would() {
        let server = orders_server().await;

        let clerk = report_json_as(&server, "/sign-in", "/back/chosen").await;

        assert_eq!(clerk, report_for_a_clerk());
    }

    #[tokio::test]
    async fn the_envelope_names_no_instant_it_withholds() {
        let server = orders_server().await;
        let screen = "application/vnd.sofie.screen+json";

        let clerk = get_as(&server, "/sign-in", "/back/paid", screen)
            .await
            .json();
        let manager = get_as(&server, "/sign-in/manager", "/back/paid", screen)
            .await
            .json();

        assert_eq!(
            clerk["data"],
            json!({ "title": "Order 7", "paid_at": null })
        );
        assert_eq!(clerk["instants"], Value::Null);
        assert_eq!(manager["instants"], json!({ "paid_at": true }));
    }

    #[tokio::test]
    async fn the_envelope_an_action_refreshes_withholds_as_the_page_does() {
        let server = orders_server().await;
        let cookie = signed_in(&server, "/sign-in").await;

        let refreshed = server
            .post("/back/report")
            .header("cookie", &cookie)
            .header("accept", "application/vnd.sofie.screen+json")
            .header("sec-fetch-site", "same-origin")
            .await;

        refreshed.assert_ok();
        assert_eq!(refreshed.json()["data"], report_for_a_clerk());
    }

    #[tokio::test]
    async fn a_permission_named_outside_a_permitted_area_is_refused_at_boot() {
        let app = App::new(Db::new(FakeDriver::default()))
            .membership(Roles)
            .area("shop", |routes: &mut Routes<Web>| {
                routes.get("/orders", list_orders).permission(LIST);
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

        let refused = app.serve(listener, async {}).await.unwrap_err();

        assert!(
            refused.to_string().contains("does not admit by permission"),
            "{refused}"
        );
    }
}

#[cfg(feature = "flags")]
mod flagged {
    use std::time::Duration;

    use serde_json::{Value, json};
    use sofie_core::Result;
    use sofie_db::{Db, Outcome, ReadOnly, Reply};
    use sofie_flags::{Flags, Rule};
    use sofie_http::{App, Gates, Routes, Web};
    use sofie_testing::TestApp;

    const CHECKOUT_SCREEN: &str = r#"<h1>{data.total}</h1>
{#if enabled("shop.split_payments")}<p>{data.split.parts}</p>{/if}"#;

    struct Checkout;

    impl Outcome for Checkout {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(
                json!({ "total": 90, "split": { "parts": 3, "due": 30 } }),
            ))
        }
    }

    impl ReadOnly for Checkout {}

    async fn checkout() -> Result<Checkout> {
        Ok(Checkout)
    }

    async fn started() -> TestApp {
        sofie_testing::start(
            |db: Db| async move {
                db.schema().migrate(&[]).await?;
                let flags = Flags::start(&db, "test").await?;
                let screens =
                    sofie_http::screens_from_sources(&[("shop/web/checkout", CHECKOUT_SCREEN)])?;
                Ok(App::new(db)
                    .screens(screens)
                    .gates(Gates::default().features(flags))
                    .area("shop", |routes: &mut Routes<Web>| {
                        routes.screen("/checkout", "checkout", checkout);
                    }))
            },
            sofie_db::worker::Worker::new,
        )
        .await
    }

    async fn checkout_json(app: &TestApp) -> Value {
        let answer = app.get("/checkout").accept_json().await;
        serde_json::from_str(&answer.body).unwrap()
    }

    #[tokio::test]
    async fn a_nested_field_behind_a_flag_is_sent_once_the_flag_is_on() {
        let app = started().await;
        assert_eq!(
            checkout_json(&app).await,
            json!({ "total": 90, "split": { "parts": null, "due": null } })
        );

        let elsewhere = Flags::start(app.db(), "test").await.unwrap();
        elsewhere
            .set("shop.split_payments", Rule::everyone())
            .await
            .unwrap();
        drop(elsewhere);
        for _ in 0..50 {
            if checkout_json(&app).await["split"]["parts"] == 3 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the split payments flag did not reach the app within five seconds");
    }
}

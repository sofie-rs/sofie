mod support;

use serde_json::json;
use sofie_testing::{TestApp, test_app};
use support::{PASSWORD, customer};

const HOME: &str = "/account/orders";

async fn sign_in_toward(app: &TestApp, next: &str) -> String {
    let signed_in = app
        .post(&format!("/sign-in?next={next}"))
        .form(&[("email", "ada@example.com"), ("password", PASSWORD)])
        .await;
    assert_eq!(signed_in.status, 303, "{}", signed_in.body);
    signed_in.header("location").to_owned()
}

/// Signing up answers alike for a new and a taken address, so it signs nobody in.
#[test_app]
async fn signing_up_signs_nobody_in_and_the_account_then_signs_in(app: TestApp) {
    let signed_up = app
        .post("/sign-up")
        .form(&[("email", "ada@example.com"), ("password", PASSWORD)])
        .await;

    assert_eq!(signed_up.status, 303, "{}", signed_up.body);
    assert!(signed_up.session().is_none());
    // A workspace build turns on sofie-identity's mailed links, which send it to check its mail.
    assert!(
        ["/sign-in", "/account/link-sent"].contains(&signed_up.header("location")),
        "{}",
        signed_up.header("location")
    );
    assert_eq!(sign_in_toward(&app, HOME).await, HOME);
}

#[test_app]
async fn signing_in_follows_next_into_an_area_of_the_account(app: TestApp) {
    customer(&app, "ada@example.com").await;

    let page = app
        .get("/sign-in?next=/account/orders%3Fpage%3D2")
        .await
        .body;
    let location = sign_in_toward(&app, "/account/orders%3Fpage%3D2").await;

    assert!(
        page.contains("action=\"/sign-in?next=/account/orders%3Fpage%3D2\""),
        "{page}"
    );
    assert_eq!(location, "/account/orders?page=2");
}

#[test_app]
async fn signing_in_follows_next_to_a_public_page(app: TestApp) {
    customer(&app, "ada@example.com").await;

    assert_eq!(sign_in_toward(&app, "/sign-up").await, "/sign-up");
}

#[test_app]
async fn a_next_leaving_the_site_or_naming_no_page_goes_home(app: TestApp) {
    customer(&app, "ada@example.com").await;

    for next in [
        "//evil.test",
        "/%5Cevil.test",
        "https://evil.test",
        "%2F%2Fevil.test",
        "%252F%252Fevil.test",
        "/%252F%252Fevil.test",
        "/%09/evil.test",
        "/nowhere",
    ] {
        assert_eq!(sign_in_toward(&app, next).await, HOME, "{next}");
    }
}

/// The second either sees the first's account and answers as for any address, or races it
/// to the unique index and only then is told the address is taken.
#[test_app]
async fn of_two_racing_sign_ups_with_one_email_one_account_is_made(app: TestApp) {
    let credentials = json!({ "email": "ada@example.com", "password": PASSWORD });

    let (first, second) = tokio::join!(
        app.post("/sign-up").json(&credentials).into_future(),
        app.post("/sign-up").json(&credentials).into_future(),
    );

    let mut statuses = [first.status.as_u16(), second.status.as_u16()];
    statuses.sort_unstable();
    assert!(
        statuses == [204, 204] || statuses == [204, 422],
        "{first:?} {second:?}"
    );
    for refused in [first, second].iter().filter(|answer| answer.status == 422) {
        assert_eq!(
            refused.json()["errors"],
            json!([{
                "field": "email",
                "key": "validation.unique",
                "arguments": {},
                "message": "is already registered",
            }])
        );
    }
    let accounts = app
        .query("SELECT count(*) FROM sofie_identity.account", &[])
        .await;
    assert_eq!(accounts, [[sofie::core::SqlValue::Int(1)]]);
}

#[test_app]
async fn sign_in_attempts_are_limited_per_client_address(app: TestApp) {
    customer(&app, "ada@example.com").await;
    let wrong = json!({ "email": "ada@example.com", "password": "guess" });
    let right = json!({ "email": "ada@example.com", "password": PASSWORD });

    for _ in 0..9 {
        assert_eq!(app.post("/sign-in").json(&wrong).await.status, 422);
    }
    let limited = app.post("/sign-in").json(&right).await;
    app.clock()
        .advance(std::time::Duration::from_secs(6))
        .unwrap();
    let later = app.post("/sign-in").json(&right).await;

    assert_eq!(limited.status, 429, "{}", limited.body);
    assert_eq!(limited.header("retry-after"), "6");
    assert_eq!(later.status, 204, "{}", later.body);
}

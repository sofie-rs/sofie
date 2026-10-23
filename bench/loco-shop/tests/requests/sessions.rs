use loco_rs::testing::prelude::*;
use loco_shop::app::App;
use serial_test::serial;

const SIGN_UP: [(&str, &str); 3] = [
    ("name", "Ada"),
    ("email", "ada@example.com"),
    ("password", "secret123"),
];

#[tokio::test]
#[serial]
async fn signing_up_signs_in_and_signing_out_signs_out() {
    request::<App, _, _>(|request, _ctx| async move {
        let signed_up = request.post("/sign-up").form(&SIGN_UP).await;
        assert_eq!(signed_up.status_code(), 303);
        assert!(signed_up.maybe_cookie("auth_token").is_some());

        let signed_out = request.post("/sign-out").await;
        assert_eq!(signed_out.status_code(), 303);

        let wrong = [("email", "ada@example.com"), ("password", "wrong")];
        assert_eq!(
            request.post("/sign-in").form(&wrong).await.status_code(),
            422
        );
        let right = [("email", "ada@example.com"), ("password", "secret123")];
        assert_eq!(
            request.post("/sign-in").form(&right).await.status_code(),
            303
        );
    })
    .await;
}

use loco_rs::controller::views::{engines, ViewRenderer};

#[test]
fn renders_sign_in_view() {
    let view = engines::TeraView::build().expect("view engine should build");

    let rendered = view
        .render("sessions/sign_in.html", serde_json::json!({}))
        .expect("sign-in view should render");

    assert!(
        rendered.contains("<h1>Sign in</h1>"),
        "expected the sign-in form, got: {rendered}"
    );
}

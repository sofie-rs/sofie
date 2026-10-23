use sofie::serde_json::json;
use sofie::values::Email;
use sofie_testing::{TestApp, TestPrincipal, TestSession, fixture_value};

#[cfg(feature = "permissions")]
use crate::Permitted;
#[cfg(feature = "permissions")]
use sofie::db::Aggregate as _;

use crate::MemberArea;

/// Long enough for any password rule; fixtures never sign in with it.
const MEMBER_PASSWORD: &str = "fixture member passphrase";

pub trait TestMember: Sized {
    fn member(app: &TestApp) -> impl Future<Output = Self>;
}

impl<A: MemberArea> TestMember for TestPrincipal<A> {
    async fn member(app: &TestApp) -> Self {
        let email: Email = fixture_value(app);
        let session = signed_up::<A>(app, email.as_str(), MEMBER_PASSWORD).await;
        app.principal_of(&session)
            .expect("the app's own session signs in to the area")
    }
}

/// Signs `email` up to `A` and then in, as a person does: signing up signs nobody in. Where
/// `A` asks for a confirmed address, the account is confirmed between the two, as the mailed
/// link would.
pub async fn signed_up<A: MemberArea>(app: &TestApp, email: &str, password: &str) -> TestSession {
    let sign_up =
        A::SIGN_UP.unwrap_or_else(|| panic!("area `{}` has no `SIGN_UP` to join it at", A::NAME));
    let credentials = json!({ "email": email, "password": password });
    app.post(sign_up).json(&credentials).await.assert_ok();
    #[cfg(feature = "email-verification")]
    if A::VERIFIED_EMAIL {
        confirm(app, email).await;
    }
    let signed_in = app.post(A::SIGN_IN).json(&credentials).await;
    signed_in.session().unwrap_or_else(|| {
        panic!(
            "signing in to `{}` after signing up answered {}: {}",
            A::NAME,
            signed_in.status,
            signed_in.body
        )
    })
}

/// Written directly rather than through the mailed link, so a test needs no worker running.
#[cfg(feature = "email-verification")]
async fn confirm(app: &TestApp, email: &str) {
    let address = Email::parse(email).expect("a signed-up address is an email");
    let mut account = crate::Account::of_email(&app.db().read(), address)
        .await
        .expect("reading the signed-up account")
        .unwrap_or_else(|| panic!("signing up as `{email}` opened no account"));
    account.confirm_email(app.db().clock().now());
    app.db()
        .commit(&mut account)
        .await
        .expect("confirming the signed-up account");
}

/// A new member of the `Permitted` area `A` whose role holds exactly `permissions`, written
/// directly, so `A` needs no sign-up; act as it with `app.as_(&principal)`.
#[cfg(feature = "permissions")]
pub async fn granted<A: MemberArea<Principal = Permitted>>(
    app: &TestApp,
    permissions: &[&str],
) -> TestPrincipal<A> {
    let db = app.db();
    let read = db.read();
    let email: Email = fixture_value(app);
    let mut role = crate::create_role(&read, &format!("granted {}", email.as_str()))
        .await
        .expect("a role named after a fresh address is free");
    db.commit(&mut role).await.expect("creating the test role");
    for permission in permissions {
        let mut grant = crate::grant(&read, role.id(), permission)
            .await
            .unwrap_or_else(|error| panic!("granting `{permission}`: {error}"));
        db.commit(&mut grant).await.expect("granting the test role");
    }
    let mut added = crate::add_member(&read, &db.clock(), email, A::NAME)
        .await
        .expect("adding a fresh address to the area")
        .with_role(&read, &db.clock(), role.id())
        .await
        .expect("giving the test member its role");
    db.commit(&mut added).await.expect("adding the test member");
    TestPrincipal::new(sofie::db::Id::from_uuid(added.account().uuid()))
}

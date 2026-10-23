use crate::TestApp;

/// What the app's mail jobs sent so far, through the fake transport: `drain` first.
#[must_use]
pub fn sent_mails(app: &TestApp) -> Vec<sofie_mail::Mail> {
    app.outbox.mails()
}

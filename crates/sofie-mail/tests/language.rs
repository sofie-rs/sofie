#![cfg(feature = "i18n")]

use std::sync::{Arc, OnceLock};

use serde_json::json;
use sofie_core::Result;
use sofie_db::worker::Worker;
use sofie_db::{Db, File, Queued};
use sofie_engine::{Components, Translations, compile_template_with_texts};
use sofie_http::{App, Routes, Web};
use sofie_mail::{Address, Mail, Mailer, Template};
use sofie_testing::{TestApp, sent_mails};
use sofie_values::{Email, Locale};

const WELCOME: &str = "<p>{t(\"welcome.greeting\", name = data.name)}</p>";
const ENGLISH: &str = "[welcome]\ngreeting = \"Welcome, {name}.\"\nsubject = \"Welcome\"\n";
const TURKISH: &str = "[welcome]\ngreeting = \"Hoş geldin, {name}.\"\nsubject = \"Hoş geldin\"\n";

fn translations() -> Translations {
    let mut translations = Translations::new(&["en", "tr"]).unwrap();
    translations.add("club", "en", ENGLISH).unwrap();
    translations.add("club", "tr", TURKISH).unwrap();
    translations
}

/// Written to disk too, as a development build rereads a mail from its source.
fn welcome() -> &'static Template {
    static WELCOME_TEMPLATE: OnceLock<Template> = OnceLock::new();
    WELCOME_TEMPLATE.get_or_init(|| {
        let bytes = compile_template_with_texts(
            "club/welcome",
            WELCOME,
            &Components::default(),
            &translations(),
        )
        .unwrap();
        let path =
            std::env::temp_dir().join(format!("sofie-mail-language-{}.html", std::process::id()));
        std::fs::write(&path, WELCOME).unwrap();
        let path: &'static str = String::leak(path.to_string_lossy().into_owned());
        Template::new("club/welcome", File::new(path, Vec::leak(bytes)))
    })
}

async fn welcome_in(mailer: Mailer, language: Option<&str>) -> Result<Queued> {
    let mailer = mailer.in_language(language.map(Locale::parse).transpose()?);
    let body = mailer.body_from(welcome(), &json!({ "name": "Ada" }))?;
    let subject = mailer.text("welcome.subject", &[], "Welcome");
    let to = Address::new(&Email::parse("ada@example.com")?)?;
    mailer.send(Mail::new(to, &subject, body)?)
}

async fn welcome_turkish(mailer: Mailer) -> Result<Queued> {
    welcome_in(mailer, Some("tr-TR")).await
}

async fn welcome_unnamed(mailer: Mailer) -> Result<Queued> {
    welcome_in(mailer, None).await
}

fn routes(routes: &mut Routes<Web>) {
    routes.post("/turkish", welcome_turkish);
    routes.post("/unnamed", welcome_unnamed);
}

async fn started(translated: bool) -> TestApp {
    sofie_testing::start(
        move |db: Db| async move {
            db.schema().migrate(&[]).await?;
            if translated {
                sofie_mail::translations(&db, Arc::new(translations()));
            }
            Ok(App::new(db).area::<Web>("club", routes))
        },
        Worker::new,
    )
    .await
}

async fn sent(app: &TestApp, path: &str) -> (String, String) {
    assert_eq!(app.post(path).await.status, 204);
    app.drain().await;
    let mails = sent_mails(app);
    let mail = mails.last().unwrap();
    (mail.subject().to_owned(), mail.body().text().to_owned())
}

#[tokio::test]
async fn a_mail_is_written_in_the_language_its_handler_names() {
    let app = started(true).await;

    assert_eq!(
        sent(&app, "/turkish").await,
        ("Hoş geldin".to_owned(), "Hoş geldin, Ada.".to_owned())
    );
    assert_eq!(
        sent(&app, "/unnamed").await,
        ("Welcome".to_owned(), "Welcome, Ada.".to_owned())
    );
}

#[tokio::test]
async fn without_the_app_s_translations_a_mail_keeps_its_compiled_texts() {
    let app = started(false).await;

    assert_eq!(
        sent(&app, "/turkish").await,
        ("Welcome".to_owned(), "Welcome, Ada.".to_owned())
    );
}

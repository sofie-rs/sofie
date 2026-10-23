#![cfg(all(feature = "i18n", feature = "islands"))]

mod common;

use serde_json::{Value, json};
use sofie_engine::rendering::Rendering;
use sofie_engine::{Submitted, Template, Translations};
use sofie_values::{Control, FormField, Input};

struct Screen {
    key: &'static str,
    source: &'static str,
    data: Value,
    ids: &'static [&'static str],
    form: Option<(&'static str, &'static str)>,
}

const CREDENTIALS: &[FormField] = &[
    FormField {
        name: "email",
        input: Input::typed(&[("type", "email")]),
        required: true,
        constraints: &[],
    },
    FormField {
        name: "password",
        input: Input::new(Control::Password),
        required: true,
        constraints: &[],
    },
];

fn screens() -> Vec<Screen> {
    let credentials = json!({"action": "/sign-in", "other": "/sign-up?next=<a>"});
    vec![
        Screen {
            key: "order/customer/orders",
            source: include_str!(
                "../../../examples/shop/modules/order/src/areas/customer/screens/orders.html"
            ),
            data: json!([
                {"id": 1, "status": "PLACED", "total": {"minor": 1250, "currency": "EUR"},
                 "status_to": {"CANCELLED": true}},
                {"id": 2, "status": "CANCELLED", "total": {"minor": 99, "currency": "EUR"},
                 "status_to": {"CANCELLED": false}},
            ]),
            ids: &["orders", "place-order", "sign-out"],
            form: Some(("place-order", "/account/orders")),
        },
        Screen {
            key: "board/web/summary",
            source: include_str!(
                "../../../examples/islands/modules/board/src/areas/web/screens/summary.html"
            ),
            data: json!({"title": "Notes <&>", "words": 3}),
            ids: &[],
            form: None,
        },
        Screen {
            key: "sofie_identity/web/sign_in",
            source: include_str!("../../sofie-identity/src/screens/sign_in.html"),
            data: credentials.clone(),
            ids: &["sign-in"],
            form: Some(("sign-in", "/sign-in")),
        },
        Screen {
            key: "sofie_identity/web/sign_up",
            source: include_str!("../../sofie-identity/src/screens/sign_up.html"),
            data: credentials,
            ids: &["sign-up"],
            form: Some(("sign-up", "/sign-in")),
        },
        Screen {
            key: "sofie_identity/console/flags",
            source: include_str!("../../sofie-identity/src/permitted/screens/flags.html"),
            data: json!([{"name": "order.split_payments", "on": true}]),
            ids: &["flags", "switch"],
            form: None,
        },
        Screen {
            key: "sofie_identity/console/shield",
            source: include_str!("../../sofie-identity/src/permitted/screens/shield.html"),
            data: json!([{"id": 1, "caller": "10.0.0.1", "rule": "sign_in", "grade": 2,
                          "until": "2026-10-12T10:00:00Z"}]),
            ids: &["blocks", "lift"],
            form: None,
        },
    ]
}

fn described(template: &mut Template, screen: &Screen) {
    if let Some((form, _)) = screen
        .form
        .filter(|_| screen.key.starts_with("sofie_identity"))
    {
        template.describe_form(form, CREDENTIALS).unwrap();
    }
}

/// Everything a request may ask of a screen, rendered from one program.
fn renders(template: &Template, screen: &Screen, translations: &Translations) -> Vec<String> {
    let mut html = Vec::new();
    for language in ["en", "de", "tr"] {
        let rendering =
            Rendering::new(template).language(translations.negotiate(None, Some(language)));
        html.push(rendering.render(&screen.data).unwrap());
        for id in screen.ids {
            html.push(rendering.render_fragment(&screen.data, id).unwrap());
            html.push(rendering.render_out_of_band(&screen.data, id).unwrap());
        }
        if let Some((_, action)) = screen.form {
            let input = json!({"email": "typed <@>", "password": "x"});
            let errors = [("email", "is taken".to_owned())];
            let submitted = Submitted {
                action,
                input: &input,
                errors: &errors,
            };
            html.push(
                rendering
                    .render_submitted(&screen.data, &submitted)
                    .unwrap(),
            );
        }
    }
    html
}

#[test]
fn every_shipped_screen_renders_the_same_from_its_compiled_program() {
    let translations = Translations::new(&["en", "de", "tr"]).unwrap();
    for screen in screens() {
        let mut parsed = Template::parse(screen.key, screen.source).unwrap();
        let mut loaded = common::loaded(screen.key, screen.source);
        described(&mut parsed, &screen);
        described(&mut loaded, &screen);

        let expected = renders(&parsed, &screen, &translations);
        assert_eq!(
            renders(&loaded, &screen, &translations),
            expected,
            "{}",
            screen.key
        );
        assert!(
            expected.iter().all(|html| !html.is_empty()),
            "{}",
            screen.key
        );
    }
}

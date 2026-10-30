#![cfg(feature = "i18n")]

mod common;

use serde_json::{Value, json};
use sofie_engine::rendering::Rendering;
use sofie_engine::{
    Gap, GapKind, KeyKind, KeyUse, Submitted, Template, Tone, TranslationError, Translations,
    check_translations, translation_keys,
};
use sofie_values::{Control, FormField, Input, Locale};

const ORDER_EN: &str = "[orders]\ntitle = \"Your orders\"\ncount = \"{count} orders for {name}\"\n";
const ORDER_TR: &str = "[orders]\ntitle = \"Siparişleriniz\"\ncount = \"{name} için {count} sipariş\"\n\n[label]\ntotal = \"Toplam\"\nstatus = \"Durum\"\n\n[enum.order_status]\npending = \"Bekliyor\"\npaid = \"Ödendi\"\n";

fn translations() -> Translations {
    let mut translations = Translations::new(&["en", "tr", "de"]).unwrap();
    translations.add("order", "en", ORDER_EN).unwrap();
    translations.add("order", "tr", ORDER_TR).unwrap();
    translations
        .add_enum(
            "order_status",
            &[("PENDING", Tone::Neutral), ("PAID", Tone::Success)],
        )
        .unwrap();
    translations
}

fn template(source: &str) -> Template {
    common::loaded("orders", source)
}

fn render_in(translations: &Translations, language: &str, source: &str, data: &Value) -> String {
    render_template_in(translations, language, &template(source), data)
}

fn render_template_in(
    translations: &Translations,
    language: &str,
    template: &Template,
    data: &Value,
) -> String {
    let language = translations.negotiate(None, Some(language));
    Rendering::new(template)
        .language(language)
        .render(data)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn described(source: &str, name: &'static str, control: Control) -> Template {
    let fields = Box::leak(Box::new([FormField {
        name,
        input: Input::new(control),
        required: true,
        constraints: &[],
    }]));
    let mut template = template(source);
    template
        .describe_form("f", fields)
        .unwrap_or_else(|error| panic!("{error}"));
    template
}

#[test]
fn t_writes_the_viewers_text_with_its_arguments_escaped() {
    let translations = translations();
    let source = "<h1>{t(\"orders.title\")}</h1><p title=\"{t(\"orders.count\", count = number(data.count), name = data.name)}\">x</p>";
    let data = json!({"count": 1234, "name": "<Ada>"});

    assert_eq!(
        render_in(&translations, "tr", source, &data),
        "<h1>Siparişleriniz</h1><p title=\"&lt;Ada&gt; için 1.234 sipariş\">x</p>"
    );
    assert_eq!(
        render_in(&translations, "de", source, &data),
        "<h1>Your orders</h1><p title=\"1.234 orders for &lt;Ada&gt;\">x</p>",
        "a key German lacks falls back to the source language, formats stay German"
    );
}

#[test]
fn a_number_argument_is_written_in_the_viewers_conventions() {
    let translations = translations();
    let source = "{t(\"orders.count\", count = data.count, name = data.name)}";
    let data = json!({"count": 1234, "name": "Ada"});

    assert_eq!(
        render_in(&translations, "tr", source, &data),
        "Ada için 1.234 sipariş"
    );
}

const ITEMS: [(&str, &str); 4] = [
    (
        "en",
        "[cart]\nitems.one = \"One item\"\nitems.other = \"{count} items\"\n",
    ),
    (
        "fr",
        "[cart]\nitems.one = \"{count} article\"\nitems.other = \"{count} articles\"\n",
    ),
    ("ja", "[cart]\nitems.other = \"{count}件\"\n"),
    (
        "pt",
        "[cart]\nitems.one = \"{count} item\"\nitems.other = \"{count} itens\"\n",
    ),
];

fn plural_translations() -> Translations {
    let mut translations = Translations::new(&["en", "fr", "ja", "pt"]).unwrap();
    for (language, source) in ITEMS {
        translations.add("cart", language, source).unwrap();
    }
    translations
}

#[test]
fn a_count_picks_the_languages_plural_form() {
    let translations = plural_translations();
    let source = "{t(\"cart.items\", count = data.count)}";
    let rendered = |language, count: f64| {
        render_in(&translations, language, source, &json!({ "count": count }))
    };

    assert_eq!(rendered("en", 1.0), "One item");
    assert_eq!(rendered("en", 0.0), "0 items");
    assert_eq!(rendered("en", 1.5), "1.5 items");
    assert_eq!(rendered("en", 1234.0), "1,234 items");
    assert_eq!(rendered("fr", 0.0), "0 article");
    assert_eq!(rendered("fr", 2.0), "2 articles");
    assert_eq!(rendered("ja", 1.0), "1件");
    assert_eq!(rendered("pt", 0.0), "0 item");
    assert_eq!(rendered("pt", 2.0), "2 itens");
}

#[test]
fn a_one_form_is_picked_by_the_languages_cldr_rule_and_reported_where_none_is() {
    let languages = ["en", "pl", "he", "sw", "hi", "fa", "ru", "sl", "ko"];
    let mut translations = Translations::new(&languages).unwrap();
    for language in languages {
        let source = "[cart]\nitems.one = \"{count}!\"\nitems.other = \"{count}\"\n";
        translations.add("cart", language, source).unwrap();
    }
    let source = "{t(\"cart.items\", count = data.count)}";
    let rendered = |language, count: i64| {
        render_in(&translations, language, source, &json!({ "count": count }))
    };

    for language in ["pl", "he", "sw", "hi", "fa", "ru"] {
        assert_eq!(rendered(language, 1), "1!", "{language}");
        assert_eq!(rendered(language, 2), "2", "{language}");
    }
    assert_eq!(rendered("hi", 0), "0!");
    assert_eq!(rendered("fa", 0), "0!");
    assert_eq!(rendered("he", 0), "0");
    assert_eq!(rendered("ru", 21), "21!");
    assert_eq!(rendered("ru", 11), "11");
    assert_eq!(rendered("ko", 1), "1");
    let uses = translation_keys(&template(source));
    let gaps: Vec<String> = translations
        .gaps(&uses)
        .iter()
        .filter(|gap| gap.key.starts_with("cart."))
        .map(Gap::to_string)
        .collect();
    assert_eq!(
        gaps,
        ["`cart.items.one` in `ko` is never shown: a count there always picks `other`"],
        "a language whose rule is only approximated, as `sl`'s 101, is not told to drop it"
    );
}

#[test]
fn a_language_without_its_own_one_form_writes_its_other_and_is_told() {
    let mut translations = Translations::new(&["en", "de"]).unwrap();
    let english = "[cart]\nitems.one = \"One item\"\nitems.other = \"{count} items\"\n";
    translations.add("cart", "en", english).unwrap();
    translations
        .add("cart", "de", "[cart]\nitems.other = \"{count} Artikel\"\n")
        .unwrap();
    let source = "{t(\"cart.items\", count = data.count)}";

    let german = render_in(&translations, "de", source, &json!({ "count": 1 }));

    assert_eq!(german, "1 Artikel");
    let uses = translation_keys(&template(source));
    let gaps: Vec<String> = translations
        .gaps(&uses)
        .iter()
        .filter(|gap| gap.key.starts_with("cart."))
        .map(Gap::to_string)
        .collect();
    assert_eq!(gaps, ["`cart.items.one` has no `de` translation"]);
    #[cfg(feature = "clients")]
    {
        let view = Rendering::new(&template(source))
            .language(translations.negotiate(None, Some("de")))
            .client_view()
            .to_value();
        assert_eq!(view["messages"].get("cart.items.one"), None);
    }
}

#[cfg(feature = "clients")]
#[test]
fn a_client_area_is_given_the_plural_forms_and_rule() {
    let translations = plural_translations();
    let screen = template("{t(\"cart.items\", count = data.n)}");
    let view = Rendering::new(&screen)
        .language(translations.negotiate(None, Some("fr")))
        .client_view()
        .to_value();

    assert_eq!(view["messages"]["cart.items.one"], "{count} article");
    assert_eq!(view["messages"]["cart.items.other"], "{count} articles");
    assert_eq!(view["conventions"]["plural"], "one-is-zero-or-one");
}

#[cfg(all(feature = "clients", feature = "gates"))]
#[test]
fn a_client_view_is_in_the_asked_language_and_gated_for_its_viewer() {
    struct Allowed(bool);
    impl sofie_engine::rendering::Gates for Allowed {
        fn can(&self, _permission: &str) -> bool {
            self.0
        }

        fn enabled(&self, _flag: &str) -> bool {
            false
        }
    }
    let screen =
        template("{#if can(\"order.order.total.read\")}x{/if}{t(\"cart.items\", count = data.n)}");
    let view = |translations: &Translations, language, allowed| {
        let gates = Allowed(allowed);
        Rendering::new(&screen)
            .gates(&gates)
            .language(translations.negotiate(None, Some(language)))
            .client_view()
            .to_value()
    };
    let items = |view: &Value| view["messages"]["cart.items.other"].clone();
    let allowed = |view: &Value| view["gates"]["permission"]["order.order.total.read"].clone();
    let translations = plural_translations();

    let french = view(&translations, "fr", true);
    let english = view(&translations, "en", false);
    let french_again = view(&translations, "fr", false);
    let mut changed = Translations::new(&["fr"]).unwrap();
    changed
        .add("cart", "fr", "[cart]\nitems.other = \"{count} choses\"\n")
        .unwrap();

    assert_eq!(items(&french), "{count} articles");
    assert_eq!(items(&english), "{count} items");
    assert_eq!(items(&french_again), "{count} articles");
    assert_eq!(items(&view(&changed, "fr", false)), "{count} choses");
    assert_eq!(allowed(&french), true);
    assert_eq!(allowed(&english), false);
    assert_eq!(allowed(&french_again), false);
}

#[test]
fn plural_forms_are_checked_against_the_call_and_each_other() {
    let translations = plural_translations();

    check_translations(
        &template("{t(\"cart.items\", count = data.n)}"),
        &translations,
    )
    .unwrap();
    let uncounted = check_translations(&template("{t(\"cart.items\")}"), &translations);
    assert!(uncounted.unwrap_err().message.contains("needs a `count`"));
    let mut wordy = Translations::new(&["en"]).unwrap();
    wordy
        .add(
            "cart",
            "en",
            "[cart]\nitems.one = \"One item\"\nitems.other = \"Items\"\n",
        )
        .unwrap();
    let wordless = check_translations(&template("{t(\"cart.items\")}"), &wordy);
    assert!(
        wordless.unwrap_err().message.contains("needs a `count`"),
        "a key with only plural forms cannot be picked without a count"
    );
    let uses = translation_keys(&template("{t(\"cart.items\", count = data.n)}"));
    let gaps: Vec<String> = translations
        .gaps(&uses)
        .iter()
        .filter(|gap| gap.key.starts_with("cart."))
        .map(Gap::to_string)
        .collect();
    assert_eq!(
        gaps,
        Vec::<String>::new(),
        "a language without a `one` form leaves it out"
    );
}

#[test]
fn a_key_no_language_has_is_written_as_it_is_and_refused_by_the_check() {
    let translations = translations();
    let screen = template("<p>\n  {t(\"orders.missing\")}</p>");

    assert_eq!(
        Rendering::new(&screen)
            .language(translations.negotiate(None, None))
            .render(&json!({}))
            .unwrap(),
        "<p>\n  orders.missing</p>"
    );
    let error = check_translations(&screen, &translations).unwrap_err();
    assert_eq!((error.line, error.column), (2, 4));
    assert!(
        error
            .message
            .contains("`orders.missing` has no text in `en`"),
        "{error}"
    );
}

#[test]
fn the_check_refuses_arguments_that_are_not_the_placeholders_and_unknown_enums() {
    let translations = translations();

    let wrong = check_translations(
        &template("{t(\"orders.count\", count = data.n)}"),
        &translations,
    );
    assert!(wrong.unwrap_err().message.contains("[\"count\", \"name\"]"));
    let unknown = check_translations(
        &template("<Badge value={data.s} enum=\"order_state\" />"),
        &translations,
    );
    assert!(
        unknown
            .unwrap_err()
            .message
            .contains("`order_state` is not a registered enum")
    );
    let valid = template(
        "{t(\"orders.count\", name = data.a, count = data.n)}<Badge value={data.s} enum=\"order_status\" />",
    );
    check_translations(&valid, &translations).unwrap();
}

#[test]
fn labels_are_humanized_in_the_source_language_and_translated_elsewhere() {
    let translations = translations();
    let source = "<Table id=\"o\" of={data.orders} columns=\"status total created_at\" />\
        <Form id=\"f\" post=\"/o\"><Field name=\"total\" /></Form>";
    let data = json!({"orders": []});

    let english = render_in(&translations, "en", source, &data);
    assert!(english.contains("<th scope=\"col\">Status</th><th scope=\"col\">Total</th><th scope=\"col\">Created at</th>"), "{english}");
    assert!(
        english.contains("<label for=\"f-total\">Total</label>"),
        "{english}"
    );
    assert!(english.contains(">No entries</td>"), "{english}");

    let turkish = render_in(&translations, "tr-TR", source, &data);
    assert!(turkish.contains("<th scope=\"col\">Durum</th><th scope=\"col\">Toplam</th><th scope=\"col\">Created at</th>"), "{turkish}");
    assert!(
        turkish.contains("<label for=\"f-total\">Toplam</label>"),
        "{turkish}"
    );
    assert!(turkish.contains(">Kayıt yok</td>"), "{turkish}");
}

#[test]
fn amounts_numbers_and_days_follow_the_viewers_conventions() {
    let translations = translations();
    let source = "<Money value={data.total} /> {money(data.total)} {number(data.ratio)} {date(data.day)} {date(data.at)}\
        <Table id=\"t\" of={data.rows} columns=\"total\" />";
    let total = json!({"minor": 123_450, "currency": "EUR"});
    let data = json!({"total": total, "ratio": 1234.5, "day": "2026-10-01", "at": "2026-10-11T09:30:00Z", "rows": [{"total": total}]});

    assert_eq!(
        render_in(&translations, "de", source, &data),
        "<span class=\"sofie-money\">1.234,50\u{a0}€</span> 1.234,50\u{a0}€ 1.234,5 01.10.2026 11.10.2026 09:30 UTC\
<table id=\"t\"><thead><tr><th scope=\"col\">Total</th></tr></thead><tbody><tr><td class=\"sofie-money\">1.234,50\u{a0}€</td></tr></tbody></table>"
    );
    assert!(render_in(&translations, "en", source, &data).starts_with("<span class=\"sofie-money\">€1,234.50</span> €1,234.50 1,234.5 Oct 1, 2026 Oct 11, 2026, 09:30 UTC"));
    let plain = template(source).render(&data).unwrap();
    assert!(plain.starts_with("<span class=\"sofie-money\">1234.50 EUR</span> 1234.50 EUR 1234.5 2026-10-01 2026-10-11 09:30 UTC"), "{plain}");
}

#[test]
fn a_local_date_time_is_dated_on_its_own_wall_clock_and_zone() {
    let translations = translations();
    let source = "{date(data.at)}";
    let data = json!({"at": {"local": "2026-10-20T09:30:00", "zone": "Europe/Istanbul"}});

    assert_eq!(
        render_in(&translations, "en", source, &data),
        "Oct 20, 2026, 09:30 Europe/Istanbul"
    );
    assert_eq!(
        render_in(&translations, "de", source, &data),
        "20.10.2026 09:30 Europe/Istanbul"
    );
    assert_eq!(
        template(source).render(&data).unwrap(),
        "2026-10-20 09:30 Europe/Istanbul"
    );
}

#[test]
fn badges_and_select_options_label_enum_variants() {
    let translations = translations();
    let source = "<Badge value={data.status} enum=\"order_status\" /><Badge value={data.other} enum=\"order_status\" />\
        <Form id=\"f\" post=\"/o\"><Field name=\"status\" options=\"order_status\" value={data.status} /></Form>";
    let data = json!({"status": "PAID", "other": "<LOST>"});
    let screen = described(source, "status", Control::Select);

    let html = render_template_in(&translations, "tr", &screen, &data);
    assert!(html.starts_with("<span class=\"sofie-badge sofie-tone-success\">Ödendi</span><span class=\"sofie-badge sofie-tone-neutral\">&lt;LOST&gt;</span>"), "{html}");
    assert!(html.contains("<option value=\"PENDING\">Bekliyor</option><option value=\"PAID\" selected>Ödendi</option>"), "{html}");
    let english = render_template_in(&translations, "en", &screen, &data);
    assert!(
        english.contains("<span class=\"sofie-badge sofie-tone-success\">Paid</span>"),
        "{english}"
    );
}

#[test]
fn a_value_s_own_refusal_is_translated_when_it_is_a_key() {
    let translations = translations();
    let turkish = translations.negotiate(Some(&Locale::parse("tr").unwrap()), Some("de"));
    let input = json!({"total": {"amount": "abc"}});
    let message = turkish.field_error("validation.invalid", &[], "validation.money.invalid");
    let errors = [("total", message)];
    let submitted = Submitted {
        action: "/o",
        input: &input,
        errors: &errors,
    };
    let form = described(
        "<Form id=\"f\" post=\"/o\"><Field name=\"total\" currency=\"EUR\" /></Form>",
        "total",
        Control::Money,
    );
    let html = Rendering::new(&form)
        .language(turkish)
        .render_submitted(&json!({}), &submitted)
        .unwrap();

    assert!(
        html.contains("aria-live=\"polite\">149,90 gibi bir tutar olmalı</p>"),
        "{html}"
    );
    assert_eq!(
        turkish.field_error("validation.invalid", &[], "must be even"),
        "must be even"
    );
    assert_eq!(turkish.locale().as_str(), "tr");
}

#[test]
fn a_money_input_names_its_currency_box_in_the_viewers_language() {
    let translations = translations();
    let form = described(
        "<Form id=\"f\" post=\"/o\"><Field name=\"total\" /></Form>",
        "total",
        Control::Money,
    );

    let html = render_template_in(&translations, "tr", &form, &json!({}));

    assert!(html.contains("aria-label=\"Para birimi\""), "{html}");
}

#[test]
fn the_language_comes_from_the_principal_then_the_header_then_the_source() {
    let translations = translations();
    let chosen = |preferred: Option<&str>, header: Option<&str>| {
        let preferred = preferred.map(|tag| Locale::parse(tag).unwrap());
        translations
            .negotiate(preferred.as_ref(), header)
            .locale()
            .as_str()
            .to_owned()
    };

    assert_eq!(chosen(Some("de-AT"), Some("tr")), "de");
    assert_eq!(chosen(Some("fr"), Some("fr, tr;q=0.5, de;q=0.9")), "de");
    assert_eq!(chosen(None, Some("es, *")), "en");
    assert_eq!(chosen(None, None), "en");
}

#[test]
fn calls_write_text_only() {
    for (source, message) in [
        ("{#if t(\"a\")}x{/if}", "cannot be a value"),
        ("<a href={t(\"a\")}>x</a>", "cannot be a value"),
        (
            "<Table id=\"t\" of={data.rows} columns=\"name\" as=\"row\" link=\"/o/{t(\"a\")}\" />",
            "cannot be a value",
        ),
        ("<Money value={money(data.x)} />", "reads data"),
        ("{shout(\"a\")}", "unknown function `shout`"),
        ("{t(\"Orders.Title\")}", "is not a translation key"),
        ("{t(data.key)}", "takes its key first"),
        (
            "{t(\"a\", n = data.x, n = data.y)}",
            "new lowercase argument name",
        ),
        ("{number(t(\"a\"))}", "not another call"),
    ] {
        let error = Template::parse("screen", source).expect_err(source);
        assert!(error.message.contains(message), "{source}: {error}");
    }
}

#[test]
fn a_template_lists_its_keys_for_the_tools() {
    let uses = translation_keys(&template(
        "{t(\"orders.count\", name = data.a, count = data.n)}<Form id=\"f\" post=\"/o\"><Field name=\"email\" /></Form>",
    ));

    assert_eq!(
        uses,
        [
            KeyUse {
                key: "orders.count".to_owned(),
                kind: KeyKind::Message {
                    arguments: vec!["count".to_owned(), "name".to_owned()]
                },
                line: 1,
                column: 2,
            },
            KeyUse {
                key: "label.email".to_owned(),
                kind: KeyKind::Label,
                line: 1,
                column: 74,
            },
        ]
    );
}

#[test]
fn gaps_list_what_a_language_lacks_or_writes_differently() {
    let mut translations = translations();
    translations
        .add(
            "billing",
            "de",
            "[orders]\ntitle = \"Ihre Bestellungen\"\ncount = \"{count} Bestellungen\"\n",
        )
        .unwrap();
    let uses = translation_keys(&template(
        "<Form id=\"f\" post=\"/o\"><Field name=\"email\" /></Form>",
    ));
    let gaps: Vec<String> = translations
        .gaps(&uses)
        .iter()
        .filter(|gap| !gap.key.starts_with("validation.") && !gap.key.starts_with("notice."))
        .map(Gap::to_string)
        .collect();

    assert_eq!(
        gaps,
        [
            "`enum.order_status.paid` has no `de` translation",
            "`enum.order_status.pending` has no `de` translation",
            "`label.email` has no `tr` translation",
            "`label.email` has no `de` translation",
            "`label.status` has no `de` translation",
            "`label.total` has no `de` translation",
            "`orders.count` in `de` has placeholders [\"count\"] where the others have [\"count\", \"name\"]",
        ]
    );
    let missing = Gap {
        key: "a".to_owned(),
        language: "tr".to_owned(),
        kind: GapKind::Missing,
    };
    assert_eq!(missing.to_string(), "`a` has no `tr` translation");
    assert!(translations.keys().contains(&"orders.title"));
}

#[test]
fn loading_refuses_what_cannot_be_served() {
    let mut translations = translations();
    let error = |result: Result<(), TranslationError>| result.unwrap_err().to_string();

    assert_eq!(
        error(translations.add("order", "fr", "")),
        "order/lang/fr.toml:1:1: the app does not declare language `fr`; it declares en, tr, de"
    );
    assert_eq!(
        error(translations.add("billing", "tr", "\n[orders]\ntitle = \"Başka\"")),
        "billing/lang/tr.toml:3:1: `orders.title` is already defined by module `order`"
    );
    assert_eq!(
        error(translations.add("billing", "en", "a = \"{x\"")),
        "billing/lang/en.toml:1:1: a `{` is never closed; placeholders are written `{name}`"
    );
    assert!(error(translations.add_enum("order_status", &[("A", Tone::Info)])).contains("twice"));
    assert!(Translations::new(&[]).is_err());
    assert!(Translations::new(&["en", "EN"]).is_err());
}

#[test]
fn an_apps_enum_replaces_the_builtin_one_of_its_name_once() {
    let mut translations = Translations::new(&["en"]).unwrap();
    let days = [("MON", Tone::Info), ("TUE", Tone::Info)];

    translations.add_enum("weekday", &days).unwrap();

    assert!(
        translations
            .add_enum("weekday", &days)
            .unwrap_err()
            .to_string()
            .contains("registered twice")
    );
}

#[test]
fn an_apps_text_replaces_a_framework_crates_and_undeclared_languages_are_skipped() {
    let mut translations = Translations::new(&["en", "tr"]).unwrap();
    translations
        .add_builtin(
            "sofie_identity",
            &[
                ("en", include_str!("../../sofie-identity/lang/en.toml")),
                ("tr", include_str!("../../sofie-identity/lang/tr.toml")),
                ("de", include_str!("../../sofie-identity/lang/de.toml")),
            ],
        )
        .unwrap();
    translations
        .add("shop", "tr", "[table]\nempty = \"Henüz sipariş yok\"")
        .unwrap();
    let source = "<Form id=\"f\" post=\"/s\"><Field name=\"email\" /></Form><Table id=\"t\" of={data.rows} columns=\"email\" />";

    let html = render_in(&translations, "tr", source, &json!({}));
    assert!(
        html.contains("<label for=\"f-email\">E-posta</label>"),
        "{html}"
    );
    assert!(html.contains(">Henüz sipariş yok</td>"), "{html}");
    assert_eq!(translations.gaps(&translation_keys(&template(source))), []);
    assert_eq!(translations.languages().len(), 2);
}

#[test]
fn a_value_is_formatted_by_its_present_impl_in_the_viewers_language() {
    let translations = translations();
    let total = sofie_values::Money::new(1_234_950, sofie_values::Currency::parse("EUR").unwrap());
    let in_language = |tag| translations.negotiate(None, Some(tag));

    assert_eq!(in_language("en").format(&total), "€12,349.50");
    assert_eq!(in_language("de").format(&total), "12.349,50\u{a0}€");
    assert_eq!(in_language("en").format(&Some(1_234_567_i64)), "1,234,567");
    assert_eq!(in_language("en").format(&None::<i64>), "");
}

#[test]
fn weekdays_are_labelled_in_the_viewers_language_and_times_written_as_they_read() {
    let translations = translations();
    let in_language = |tag| translations.negotiate(None, Some(tag));
    let source = "<Badge value={data.day} enum=\"weekday\" />\
        <Form id=\"f\" post=\"/s\"><Field name=\"day\" options=\"weekday\" value={data.day} /></Form>";
    let screen = described(source, "day", Control::Select);

    assert_eq!(
        in_language("tr").format(&sofie_values::Weekday::Monday),
        "Pazartesi"
    );
    assert_eq!(
        in_language("de").format(&sofie_values::Weekday::Sunday),
        "Sonntag"
    );
    assert_eq!(
        in_language("en").format(&sofie_values::TimeOfDay::constant("09:30")),
        "09:30"
    );
    let html = render_template_in(&translations, "de", &screen, &json!({"day": "FRIDAY"}));
    assert!(
        html.starts_with("<span class=\"sofie-badge sofie-tone-neutral\">Freitag</span>"),
        "{html}"
    );
    assert!(
        html.contains("<option value=\"FRIDAY\" selected>Freitag</option>"),
        "{html}"
    );
}

#[test]
fn a_field_error_is_translated_with_its_arguments() {
    let translations = translations();
    let max = [("max", "120".to_owned())];

    assert_eq!(
        translations.negotiate(None, Some("tr")).field_error(
            "validation.max",
            &max,
            "must be at most 120"
        ),
        "en fazla 120 olmalı"
    );
    assert_eq!(
        translations.negotiate(None, Some("en")).field_error(
            "validation.sku.invalid",
            &[],
            "is not a SKU"
        ),
        "is not a SKU"
    );
    assert_eq!(
        translations.negotiate(None, Some("tr")).field_error(
            "validation.unique",
            &[],
            "is already registered"
        ),
        "is already registered"
    );
}

#[test]
fn a_badge_reads_its_value_from_data() {
    for source in [
        "<Badge value=\"PAID\" enum=\"order_status\" />",
        "<Badge value={\"PAID\"} enum=\"order_status\" />",
    ] {
        let error = Template::parse("screen", source).expect_err(source);
        assert!(
            error.message.contains("`value` on `Badge` reads data"),
            "{source}: {error}"
        );
    }
}

#[cfg(feature = "listing")]
#[test]
fn a_listing_writes_a_refused_filter_in_the_viewers_language() {
    let translations = translations();
    let source = r#"<Table id="orders" of={data} columns="status" filter="status" />"#;
    let page = json!({
        "items": [],
        "page": 1,
        "errors": {
            "status": {
                "key": "validation.one_of",
                "arguments": {"values": "OPEN, PAID"},
                "message": "must be one of: OPEN, PAID",
            },
            "total.gt": {
                "key": "validation.kind.whole_number",
                "arguments": {},
                "message": "must be a whole number",
            },
        },
    });

    let html = render_in(&translations, "tr", source, &page);

    assert!(
        html.contains(">şunlardan biri olmalı: OPEN, PAID</span>"),
        "{html}"
    );
    assert!(html.contains(">total.gt: tam sayı olmalı</p>"), "{html}");
}

#[cfg(feature = "listing")]
#[test]
fn a_listing_writes_its_controls_in_the_viewers_language() {
    let translations = translations();
    let source = r#"<Table id="orders" of={data} columns="email" filter="email" />"#;
    let page = json!({"items": [], "page": 2, "has_next": true});

    for (language, words) in [
        ("tr", [">Filtrele</button>", ">Önceki</a>", ">Sonraki</a>"]),
        ("de", [">Filtern</button>", ">Zurück</a>", ">Weiter</a>"]),
        ("en", [">Filter</button>", ">Previous</a>", ">Next</a>"]),
    ] {
        let html = render_in(&translations, language, source, &page);
        for word in words {
            assert!(html.contains(word), "{language}: {word} in {html}");
        }
    }
}

#[test]
fn a_screen_compiled_with_its_texts_writes_them_where_the_language_has_none() {
    use sofie_engine::{Components, compile_template_with_texts, load_template};
    let mut source = Translations::new(&["en"]).unwrap();
    source
        .add(
            "order",
            "en",
            "[orders]\nheading = \"Orders of {name}\"\nshown.one = \"One order\"\nshown.other = \"{count} orders\"\n",
        )
        .unwrap();
    let compiled = compile_template_with_texts(
        "orders",
        "<h1>{t(\"orders.heading\", name = data.name)}</h1>\
         <p>{t(\"orders.shown\", count = data.count)}</p>",
        &Components::default(),
        &source,
    )
    .unwrap();
    let loaded = load_template("orders", &compiled).unwrap();

    assert_eq!(
        loaded
            .render(&json!({"name": "<Ada>", "count": 1}))
            .unwrap(),
        "<h1>Orders of &lt;Ada&gt;</h1><p>One order</p>"
    );
    assert_eq!(
        loaded.render(&json!({"name": "Ada", "count": 3})).unwrap(),
        "<h1>Orders of Ada</h1><p>3 orders</p>"
    );
}

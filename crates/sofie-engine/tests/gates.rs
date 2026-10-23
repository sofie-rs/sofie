#![cfg(feature = "gates")]

mod common;

use serde_json::json;
use sofie_engine::Template;
use sofie_engine::rendering::{Gates, Rendering};

/// Holds one permission, by wildcard, and one flag.
struct Console;

impl Gates for Console {
    fn can(&self, permission: &str) -> bool {
        permission.starts_with("order.order.")
    }

    fn enabled(&self, flag: &str) -> bool {
        flag == "order.split_payments"
    }
}

const SCREEN: &str = r#"<p>{#if can("order.order.total.read")}<b id="total">{data.total}</b>{/if}</p><p>{#if can("crm.member.*.read")}<i id="member">{data.member}</i>{:else}hidden{/if}</p>{#if enabled("order.split_payments")}<span>split</span>{/if}{#if enabled("order.other")}<span>other</span>{/if}"#;

#[test]
fn a_condition_shows_what_the_viewer_may_see_and_nothing_else() {
    let template = common::loaded("orders", SCREEN);
    let data = json!({"total": 5, "member": "Ada"});

    let html = Rendering::new(&template)
        .gates(&Console)
        .render(&data)
        .unwrap();

    assert_eq!(
        html,
        "<p><b id=\"total\">5</b></p><p>hidden</p><span>split</span>"
    );
    assert!(
        Rendering::new(&template)
            .gates(&Console)
            .render_fragment(&data, "member")
            .is_err(),
        "a hidden part is not reached by asking for its id"
    );
}

#[test]
fn without_a_request_nothing_gated_is_shown() {
    let template = common::loaded("orders", SCREEN);

    let html = template.render(&json!({"total": 5})).unwrap();

    assert_eq!(html, "<p></p><p>hidden</p>");
}

#[test]
fn a_condition_names_a_permission_or_a_flag_as_a_literal() {
    for (source, message) in [
        (
            r#"{#if can("order.read")}x{/if}"#,
            "`can` takes a quoted permission",
        ),
        (
            "{#if enabled(data.flag)}x{/if}",
            "`enabled` takes a quoted flag",
        ),
    ] {
        let error = Template::parse("orders", source).unwrap_err();
        assert!(error.message.contains(message), "{error}");
    }
}

/// Holds one entitlement and nothing else.
struct Starter;

impl Gates for Starter {
    fn can(&self, _permission: &str) -> bool {
        false
    }

    fn enabled(&self, _flag: &str) -> bool {
        false
    }

    fn entitled(&self, entitlement: &str) -> bool {
        entitlement == "project.export"
    }
}

#[test]
fn an_entitlement_shows_what_the_viewers_plan_holds() {
    let template = common::loaded(
        "projects",
        r#"{#if entitled("project.export")}<a>export</a>{/if}{#if entitled("project.audit")}<a>audit</a>{:else}upgrade{/if}"#,
    );

    let html = Rendering::new(&template)
        .gates(&Starter)
        .render(&json!({}))
        .unwrap();

    assert_eq!(html, "<a>export</a>upgrade");
    assert!(
        Template::parse("projects", r#"{#if entitled("export")}x{/if}"#)
            .unwrap_err()
            .message
            .contains("`entitled` takes a quoted entitlement")
    );
}

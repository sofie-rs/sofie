//! A client area's pages: one shell holding the first screen as JSON, and the area's bundle,
//! which renders every later screen from its envelope.

use crate::asset::{Asset, CSS, JAVASCRIPT, STYLESHEET};
use crate::escape::{push_escaped, push_script_json};
use crate::page::IslandLinks;

#[derive(Clone, Copy, Debug)]
pub struct ClientBundle {
    pub area: &'static str,
    /// The screens compiled into it; any other screen of the area stays an HTML page.
    pub screens: &'static [&'static str],
    pub script: Asset,
    pub stylesheet: Asset,
}

impl ClientBundle {
    #[must_use]
    pub const fn new(
        area: &'static str,
        screens: &'static [&'static str],
        script: &'static [u8],
        stylesheet: &'static [u8],
    ) -> Self {
        Self {
            area,
            screens,
            script: Asset::new(area, "client.js", script, JAVASCRIPT),
            stylesheet: Asset::new(area, "client.css", stylesheet, CSS),
        }
    }

    #[must_use]
    pub fn has_screen(&self, key: &str) -> bool {
        self.screens.contains(&key)
    }

    #[must_use]
    pub fn links(&self) -> ClientLinks {
        ClientLinks {
            script: self.script.path(),
            stylesheet: self.stylesheet.path(),
        }
    }
}

/// Where the shell loads the bundle from: the embedded one, or the one `sofie dev` rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientLinks {
    pub script: String,
    pub stylesheet: String,
}

/// The area's bundle the app's build script wrote to `OUT_DIR/clients`, with its compressed
/// copies when built with them.
#[macro_export]
macro_rules! client_bundle {
    ($area:literal, $screens:expr) => {{
        let bundle = $crate::ClientBundle::new(
            $area,
            $screens,
            include_bytes!(concat!(env!("OUT_DIR"), "/clients/", $area, ".js")),
            include_bytes!(concat!(env!("OUT_DIR"), "/clients/", $area, ".css")),
        );
        $crate::ClientBundle {
            script: $crate::compressed_asset!(
                bundle.script,
                concat!(env!("OUT_DIR"), "/clients/", $area, ".js")
            ),
            stylesheet: $crate::compressed_asset!(
                bundle.stylesheet,
                concat!(env!("OUT_DIR"), "/clients/", $area, ".css")
            ),
            ..bundle
        }
    }};
}

pub const SCREEN_ELEMENT: &str = "sofie-screen";

#[derive(Debug)]
pub struct ClientShell<'a> {
    pub title: &'a str,
    pub lang: &'a str,
    pub nonce: &'a str,
    pub envelope_json: &'a str,
    pub links: &'a ClientLinks,
    pub islands: Option<&'a IslandLinks>,
}

impl ClientShell<'_> {
    #[must_use]
    pub fn render(&self) -> String {
        let mut html = String::with_capacity(1024);
        html.push_str("<!doctype html><html lang=\"");
        push_escaped(&mut html, self.lang);
        html.push_str(
            "\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>",
        );
        push_escaped(&mut html, self.title);
        html.push_str("</title>");
        html.push_str("<link rel=\"stylesheet\" href=\"");
        STYLESHEET.push_path(&mut html);
        html.push_str("\"><link rel=\"stylesheet\" href=\"");
        push_escaped(&mut html, &self.links.stylesheet);
        html.push_str("\">");
        if let Some(islands) = self.islands {
            islands.push_head(&mut html, self.nonce);
        }
        html.push_str("<script type=\"module\" src=\"");
        push_escaped(&mut html, &self.links.script);
        html.push_str("\" nonce=\"");
        push_escaped(&mut html, self.nonce);
        html.push_str("\"></script></head><body><div id=\"sofie-app\"></div>");
        html.push_str("<script type=\"application/json\" id=\"");
        html.push_str(SCREEN_ELEMENT);
        html.push_str("\">");
        push_script_json(&mut html, self.envelope_json);
        html.push_str("</script></body></html>");
        html
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ClientBundle, ClientShell};

    #[test]
    fn the_shell_carries_the_screen_as_inert_json_and_loads_the_bundle() {
        let bundle = ClientBundle::new("member", &["order/member/orders"], b"x", b"y");
        let envelope =
            json!({"screen": "order/member/orders", "data": {"note": "</script><b>"}}).to_string();
        let html = ClientShell {
            title: "Orders",
            lang: "en",
            nonce: "n1",
            envelope_json: &envelope,
            links: &bundle.links(),
            islands: None,
        }
        .render();

        assert!(html.contains("<title>Orders</title>"), "{html}");
        assert!(html.contains(&format!(
            "<script type=\"module\" src=\"{}\" nonce=\"n1\"></script>",
            bundle.script.path()
        )));
        assert!(
            html.contains("\\u003c/script\\u003e\\u003cb\\u003e"),
            "{html}"
        );
        assert!(bundle.has_screen("order/member/orders"));
        assert!(!bundle.has_screen("order/member/other"));
    }
}

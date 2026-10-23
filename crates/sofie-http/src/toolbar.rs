mod actions;
mod overrides;

use http::HeaderMap;
use sofie_db::Db;
use tracing::Span;

pub(crate) use overrides::Overrides;

use crate::Access;
use crate::cookie::CookieNames;
use crate::routes::Route;
use crate::session::RequestSession;

pub(crate) struct Toolbar {
    /// Those with a principal to act as.
    areas: Vec<&'static str>,
    cookies: CookieNames,
}

impl Toolbar {
    pub(crate) fn new(routes: &[Route], cookies: CookieNames) -> Self {
        let mut areas: Vec<&'static str> = routes
            .iter()
            .filter(|route| route.audience.access != Access::Public)
            .map(|route| route.audience.name)
            .collect();
        areas.sort_unstable();
        areas.dedup();
        Self { areas, cookies }
    }

    /// Needs no script or style the CSP would refuse.
    pub(crate) fn html(&self, request: &Span, headers: &HeaderMap) -> String {
        let overrides = Overrides::of(headers, self.cookies.toolbar());
        let tree = sofie_telemetry::development::span_tree(request).unwrap_or_else(|| {
            "no span tree: the framework's subscriber is not installed".to_owned()
        });
        let mut html = String::from(
            "<details id=\"sofie-toolbar\" class=\"sofie-toolbar\"><summary>Request telemetry",
        );
        push_summary(&mut html, &overrides);
        html.push_str("</summary><pre>");
        sofie_engine::push_escaped(&mut html, &tree);
        html.push_str("</pre>");
        self.push_acting(&mut html, &overrides);
        push_flags(&mut html, &overrides);
        push_clock(&mut html, &overrides);
        html.push_str("</details>");
        html
    }

    fn push_acting(&self, html: &mut String, overrides: &Overrides) {
        if self.areas.is_empty() {
            return;
        }
        open_form(html);
        html.push_str("<label>Area <select name=\"area\">");
        for area in &self.areas {
            html.push_str("<option>");
            sofie_engine::push_escaped(html, area);
            html.push_str("</option>");
        }
        html.push_str(
            "</select></label> <label>Principal id <input name=\"principal\" required></label> \
             <button name=\"action\" value=\"act_as\">Act as</button>",
        );
        if overrides.acting_as.is_some() {
            html.push_str(" <button name=\"action\" value=\"stop_acting\" formnovalidate>Stop acting</button>");
        }
        html.push_str("</form>");
    }
}

/// Visible while the toolbar is closed, so an override is never forgotten.
fn push_summary(html: &mut String, overrides: &Overrides) {
    if let Some(acting) = &overrides.acting_as {
        html.push_str(" · <strong>acting as ");
        sofie_engine::push_escaped(html, &format!("{} {}", acting.area, acting.principal));
        html.push_str("</strong>");
    }
    for (name, answer) in overrides.flags.iter() {
        html.push_str(" · flag ");
        sofie_engine::push_escaped(html, &format!("{name}={answer}"));
    }
    if let Some(seconds) = overrides.clock_seconds {
        html.push_str(" · clock ");
        sofie_engine::push_escaped(html, &format!("{seconds:+} s"));
    }
}

fn push_flags(html: &mut String, overrides: &Overrides) {
    open_form(html);
    html.push_str(
        "<label>Flag <input name=\"flag\" required placeholder=\"module.flag\"></label> \
         <label>Answer <input name=\"answer\" required placeholder=\"on, off or a variant\"></label> \
         <button name=\"action\" value=\"flag\">Override</button>",
    );
    if !overrides.flags.is_empty() {
        html.push_str(" <button name=\"action\" value=\"clear_flags\" formnovalidate>Clear flag overrides</button>");
    }
    html.push_str("</form>");
}

fn push_clock(html: &mut String, overrides: &Overrides) {
    open_form(html);
    html.push_str(
        "<label>Clock at <input name=\"at\" placeholder=\"2027-01-01T09:00 or 2027-01-01T09:00+03:00\"></label> \
         <button name=\"action\" value=\"set_clock\">Set</button> \
         <label>Shift by hours <input name=\"hours\" type=\"number\"></label> \
         <button name=\"action\" value=\"shift_clock\">Shift</button>",
    );
    if overrides.clock_seconds.is_some() {
        html.push_str(" <button name=\"action\" value=\"reset_clock\">Reset clock</button>");
    }
    html.push_str("</form>");
}

fn open_form(html: &mut String) {
    html.push_str("<form method=\"post\" action=\"");
    html.push_str(actions::PATH);
    html.push_str("\">");
}

/// Handlers, loaders and the unit of work all take the request's clock from its `Db`.
pub(crate) fn clocked(db: &Db, session: &RequestSession) -> Db {
    match session.toolbar.clock_seconds {
        Some(seconds) => db.clone().with_clock(db.clock().shifted(seconds)),
        None => db.clone(),
    }
}

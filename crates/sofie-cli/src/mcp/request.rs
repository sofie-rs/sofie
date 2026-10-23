use std::fmt::Write as _;

use super::ToolOutput;
use super::session::DevSession;
use crate::http_client::{self, Answer, Request, Response};
use crate::local_app::wait_for_trees;

const SHOWN_HEADERS: [&str; 3] = ["location", "content-type", "x-request-id"];

pub(super) struct Call<'call> {
    pub(super) method: &'call str,
    pub(super) path: &'call str,
    pub(super) body: Option<&'call str>,
    pub(super) answer: Option<&'call str>,
    pub(super) target: Option<&'call str>,
    pub(super) session: Option<&'call str>,
}

struct Traced<'response> {
    trace_id: &'response str,
    trees: Vec<String>,
}

pub(super) fn request(session: &mut DevSession, call: &Call) -> ToolOutput {
    let answer_headers = match answer_headers(call.answer, call.target) {
        Ok(headers) => headers,
        Err(error) => return ToolOutput::failed(error),
    };
    let method = call.method.to_ascii_uppercase();
    if let Err(error) = http_client::check_request_line(&method, call.path) {
        return ToolOutput::failed(error.to_string());
    }
    let address = match session.current() {
        Ok(address) => address,
        Err(error) => return ToolOutput::failed(format!("the app is not running: {error}")),
    };
    let mut headers = vec![("origin", format!("http://{address}"))];
    headers.extend(answer_headers);
    if let Some(cookie) = call
        .session
        .and_then(|name| session.jars.cookie_header(name))
    {
        headers.push(("cookie", cookie));
    }
    let body = call.body.unwrap_or_default();
    if !body.is_empty() {
        headers.push(("content-type", content_type(body).to_owned()));
    }
    let sent = Request {
        method: &method,
        path: call.path,
        headers,
        body: body.as_bytes(),
    };
    let response = match http_client::send(address, &sent) {
        Ok(response) => response,
        Err(error) => return ToolOutput::failed(error.to_string()),
    };
    let cookies = match call.session {
        Some(name) => session.jars.store(name, response.headers("set-cookie")),
        None => response
            .headers("set-cookie")
            .filter_map(cookie_name)
            .collect(),
    };
    let traced = response.trace_id().map(|trace_id| Traced {
        trace_id,
        trees: wait_for_trees(&session.log, trace_id),
    });
    ToolOutput::done(describe(&response, call.session, &cookies, traced.as_ref()))
}

fn answer_headers(
    name: Option<&str>,
    target: Option<&str>,
) -> Result<Vec<(&'static str, String)>, String> {
    let name = name.unwrap_or(if target.is_some() { "fragment" } else { "page" });
    let answer = Answer::ALL
        .into_iter()
        .find(|answer| answer.name() == name)
        .ok_or_else(|| format!("`answer` is `page`, `json` or `fragment`, not `{name}`"))?;
    let mut headers = answer.headers();
    match (answer, target) {
        (Answer::Fragment, Some(target)) => headers.push(("hx-target", target.to_owned())),
        (_, Some(_)) => {
            return Err(format!(
                "`target` names a fragment, so `answer` is `fragment`, not `{name}`"
            ));
        }
        (_, None) => {}
    }
    Ok(headers)
}

fn content_type(body: &str) -> &'static str {
    if body.trim_start().starts_with('{') {
        return "application/json";
    }
    "application/x-www-form-urlencoded"
}

fn cookie_name(set_cookie: &str) -> Option<String> {
    let (name, _) = set_cookie.split_once('=')?;
    Some(name.trim().to_owned())
}

fn describe(
    response: &Response,
    session: Option<&str>,
    cookies: &[String],
    traced: Option<&Traced>,
) -> String {
    let mut text = format!("HTTP {} {}\n", response.status, response.reason);
    text.extend(input_hint(response.status));
    for name in SHOWN_HEADERS {
        for value in response.headers(name) {
            let _ = writeln!(text, "{name}: {value}");
        }
    }
    if !cookies.is_empty() {
        let kept = match session {
            Some(session) => format!("kept in session `{session}`"),
            None => "not kept: pass `session` to keep them".to_owned(),
        };
        let _ = writeln!(text, "cookies set: {} ({kept})", cookies.join(", "));
    }
    let body = String::from_utf8_lossy(&response.body);
    let _ = writeln!(text, "\nbody ({} bytes):\n{body}", response.body.len());
    match traced {
        Some(traced) => {
            let _ = writeln!(text, "\ntrace {}", traced.trace_id);
            if traced.trees.is_empty() {
                text.push_str("no span tree logged yet; `traces` with this trace shows it later\n");
            }
            text.push_str(&traced.trees.join("\n\n"));
        }
        None => text.push_str("\nthe answer names no trace\n"),
    }
    text
}

fn input_hint(status: u16) -> Option<&'static str> {
    matches!(status, 400 | 422).then_some(
        "if a field was refused, the `explain` tool on the route names the fields it takes and \
         their JSON shape\n",
    )
}

#[cfg(test)]
mod tests {
    use super::{Traced, answer_headers, describe, input_hint};
    use crate::http_client::parse;

    #[test]
    fn a_long_body_comes_whole_and_before_the_traces() {
        let body = "b".repeat(20 * 1024);
        let answer = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = parse(answer.as_bytes(), false).unwrap();
        let traced = Traced {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
            trees: vec!["t".repeat(20 * 1024)],
        };

        let text = describe(&response, None, &[], Some(&traced));

        assert!(text.contains(&body), "the body was cut");
        assert!(text.find("body (") < text.find("trace 4bf9"), "{text:.200}");
    }

    #[test]
    fn a_target_asks_for_that_fragment() {
        let fragment = answer_headers(None, Some("orders")).unwrap();

        assert!(fragment.contains(&("hx-request", "true".to_owned())));
        assert!(fragment.contains(&("hx-target", "orders".to_owned())));
        assert!(
            !answer_headers(Some("fragment"), None)
                .unwrap()
                .iter()
                .any(|(name, _)| *name == "hx-target")
        );
        assert!(
            answer_headers(Some("json"), Some("orders"))
                .unwrap_err()
                .contains("`answer` is `fragment`")
        );
    }

    #[test]
    fn a_refused_input_points_to_where_the_route_s_input_is_described() {
        assert!(input_hint(422).is_some_and(|hint| hint.contains("`explain`")));
        assert!(input_hint(400).is_some());
        assert_eq!(input_hint(303), None);
    }
}

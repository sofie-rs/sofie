//! The app's side of `sofie dev`, in debug builds only; see `docs/structure.md`.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY};
use http::request::Parts;
use http::{HeaderMap, HeaderValue, Method, StatusCode};

use crate::answer::{self, Response, TEXT};

const DIRECTORY: &str = "SOFIE_DEV_DIR";
const RELOAD: &str = "/_sofie/dev/reload";
const ISLANDS: &str = "/_sofie/dev/islands/";
const CLIENTS: &str = "/_sofie/dev/clients/";
/// Short, so a stopping server's graceful shutdown never waits on it.
const HOLD: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

/// Answered before tracing, so the client's polling prints no span trees.
pub(crate) async fn answer(request: &Parts) -> Option<Response> {
    if request.method != Method::GET {
        return None;
    }
    let path = request.uri.path();
    if path == sofie_engine::development::DEVELOPMENT_SCRIPT.path() {
        let script = sofie_engine::development::DEVELOPMENT_SCRIPT;
        let response = answer::response(StatusCode::OK, Some(script.content_type), script.bytes);
        return Some(no_store(response));
    }
    if path == RELOAD {
        let after = request
            .uri
            .query()
            .and_then(|query| query.strip_prefix("after="))
            .unwrap_or_default();
        return Some(reload(after).await);
    }
    if let Some(name) = path.strip_prefix(ISLANDS) {
        return Some(built_file("islands", name));
    }
    path.strip_prefix(CLIENTS)
        .map(|name| built_file("clients", name))
}

async fn reload(after: &str) -> Response {
    let started = std::time::Instant::now();
    loop {
        let (generation, kind) = generation();
        if generation != after || started.elapsed() >= HOLD {
            let body = format!("{} {generation} {kind}", boot());
            return no_store(answer::response(StatusCode::OK, Some(TEXT), body));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn generation() -> (String, String) {
    let written = directory()
        .and_then(|directory| fs::read_to_string(directory.join("generation")).ok())
        .unwrap_or_default();
    let mut words = written.split_whitespace();
    let generation = words.next().unwrap_or("0").to_owned();
    let kind = words.next().unwrap_or("page").to_owned();
    (generation, kind)
}

/// Differs between runs, so a client tells a restart.
fn boot() -> &'static str {
    static BOOT: OnceLock<String> = OnceLock::new();
    BOOT.get_or_init(|| {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{:x}-{started:x}", std::process::id())
    })
}

/// An area's script or stylesheet `sofie dev` built into `kind`.
fn built_file(kind: &str, name: &str) -> Response {
    let content_type = match name.rsplit_once('.') {
        Some((area, "js")) if is_area(area) => "application/javascript; charset=utf-8",
        Some((area, "css")) if is_area(area) => "text/css; charset=utf-8",
        _ => return not_found(),
    };
    let Some(bytes) =
        directory().and_then(|directory| fs::read(directory.join(kind).join(name)).ok())
    else {
        return not_found();
    };
    no_store(answer::response(StatusCode::OK, Some(content_type), bytes))
}

#[cfg(feature = "islands")]
pub(crate) fn island_links(area: &str) -> Option<sofie_engine::islands::IslandLinks> {
    let built = directory()?.join("islands").join(format!("{area}.js"));
    built.exists().then(|| sofie_engine::islands::IslandLinks {
        script: format!("{ISLANDS}{area}.js"),
        stylesheet: format!("{ISLANDS}{area}.css"),
    })
}

#[cfg(feature = "clients")]
pub(crate) fn client_links(area: &str) -> Option<sofie_engine::ClientLinks> {
    let built = directory()?.join("clients").join(format!("{area}.js"));
    built.exists().then(|| sofie_engine::ClientLinks {
        script: format!("{CLIENTS}{area}.js"),
        stylesheet: format!("{CLIENTS}{area}.css"),
    })
}

pub(crate) fn client_script(headers: &HeaderMap) -> Option<String> {
    let policy = headers.get(CONTENT_SECURITY_POLICY)?.to_str().ok()?;
    let nonce = policy.split("'nonce-").nth(1)?.split('\'').next()?;
    Some(format!(
        "<script src=\"{}\" nonce=\"{nonce}\" defer></script>",
        sofie_engine::development::DEVELOPMENT_SCRIPT.path()
    ))
}

fn directory() -> Option<PathBuf> {
    std::env::var_os(DIRECTORY).map(PathBuf::from)
}

fn is_area(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn not_found() -> Response {
    answer::response(StatusCode::NOT_FOUND, Some(TEXT), "not found")
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_carries_the_nonce_of_the_pages_policy() {
        let mut headers = HeaderMap::new();
        let policy = crate::content_security::AreaContentSecurity::default();
        headers.insert(CONTENT_SECURITY_POLICY, policy.header("abc123").unwrap());

        let script = client_script(&headers).unwrap();

        assert_eq!(
            script,
            format!(
                "<script src=\"{}\" nonce=\"abc123\" defer></script>",
                sofie_engine::development::DEVELOPMENT_SCRIPT.path()
            )
        );
        assert_eq!(client_script(&HeaderMap::new()), None);
    }

    #[test]
    fn only_an_areas_script_and_stylesheet_are_served() {
        for kind in ["islands", "clients"] {
            for name in ["../secret.js", "member.html", ".js", "Member.js", "a/b.css"] {
                assert_eq!(
                    built_file(kind, name).status(),
                    StatusCode::NOT_FOUND,
                    "{name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_client_behind_the_generation_is_answered_at_once() {
        let started = std::time::Instant::now();

        let response = reload("").await;

        assert!(started.elapsed() < HOLD);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    }
}

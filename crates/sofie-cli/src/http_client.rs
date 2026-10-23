use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Write as _};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(2);

pub(crate) struct Request<'r> {
    pub(crate) method: &'r str,
    pub(crate) path: &'r str,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: &'r [u8],
}

#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) reason: String,
    headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum HttpError {
    Io(io::Error),
    Refused(String),
    Malformed(&'static str),
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "the request to the app failed: {error}"),
            Self::Refused(reason) => formatter.write_str(reason),
            Self::Malformed(what) => write!(formatter, "the app's answer is malformed: {what}"),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<io::Error> for HttpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl Response {
    /// Names are compared in lower case, as HTTP's are case-insensitive.
    pub(crate) fn headers<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'s str> {
        self.headers
            .iter()
            .filter(move |(found, _)| found == name)
            .map(|(_, value)| value.as_str())
    }

    /// The app answers every request with its own `traceparent`.
    pub(crate) fn trace_id(&self) -> Option<&str> {
        let traceparent = self.headers("traceparent").next()?;
        let mut parts = traceparent.split('-');
        let _version = parts.next()?;
        parts.next().filter(|id| id.len() == 32)
    }
}

pub(crate) fn send(address: SocketAddr, request: &Request) -> Result<Response, HttpError> {
    let head = head(address, request)?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let written = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(request.body));
    let mut answer = Vec::new();
    let read = stream.read_to_end(&mut answer);
    // A server may answer (413, 429) before reading the whole body, then
    // reset the connection on the rest; its answer still counts.
    let parsed = parse(&answer, request.method == "HEAD");
    if parsed.is_ok() {
        return parsed;
    }
    written?;
    read?;
    parsed
}

/// What a request line may hold, checked before anything is started or sent.
pub(crate) fn check_request_line(method: &str, path: &str) -> Result<(), HttpError> {
    if method.is_empty() || !method.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(HttpError::Refused(format!(
            "`{method}` is not an HTTP method"
        )));
    }
    if !path.starts_with('/') || !path.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(HttpError::Refused(format!(
            "the path `{}` starts with `/` and holds no space or control character",
            path.escape_debug()
        )));
    }
    Ok(())
}

fn head(address: SocketAddr, request: &Request) -> Result<String, HttpError> {
    check_request_line(request.method, request.path)?;
    let printable = |text: &str| {
        text.bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    };
    let mut head = format!(
        "{} {} HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\ncontent-length: {}\r\n",
        request.method,
        request.path,
        request.body.len()
    );
    for (name, value) in &request.headers {
        if !printable(value) {
            return Err(HttpError::Refused(format!(
                "the header `{name}` holds a control character"
            )));
        }
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    Ok(head)
}

pub(crate) fn parse(answer: &[u8], head_only: bool) -> Result<Response, HttpError> {
    let end = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(HttpError::Malformed("no end of the headers"))?;
    let head = std::str::from_utf8(&answer[..end])
        .map_err(|_| HttpError::Malformed("headers not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let (Some(version), Some(status)) = (parts.next(), parts.next()) else {
        return Err(HttpError::Malformed("no status line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Malformed("not HTTP/1"));
    }
    let status: u16 = status
        .parse()
        .map_err(|_| HttpError::Malformed("no status code"))?;
    let headers = lines
        .map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
                .ok_or(HttpError::Malformed("a header without `:`"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut response = Response {
        status,
        reason: parts.next().unwrap_or_default().to_owned(),
        headers,
        body: Vec::new(),
    };
    if !head_only && !matches!(status, 100..=199 | 204 | 304) {
        response.body = body(&response, &answer[end + 4..])?;
    }
    Ok(response)
}

fn body(response: &Response, rest: &[u8]) -> Result<Vec<u8>, HttpError> {
    let chunked = response
        .headers("transfer-encoding")
        .any(|value| value.to_ascii_lowercase().contains("chunked"));
    if chunked {
        return dechunk(rest);
    }
    let Some(length) = response.headers("content-length").next() else {
        return Ok(rest.to_vec());
    };
    let length: usize = length
        .parse()
        .map_err(|_| HttpError::Malformed("a content-length that is not a number"))?;
    rest.get(..length)
        .map(<[u8]>::to_vec)
        .ok_or(HttpError::Malformed(
            "a body shorter than its content-length",
        ))
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or(HttpError::Malformed("a chunk without its size line"))?;
        let size_line = std::str::from_utf8(&rest[..line_end])
            .map_err(|_| HttpError::Malformed("a chunk size not UTF-8"))?;
        let digits = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(digits, 16)
            .map_err(|_| HttpError::Malformed("a chunk size that is not hexadecimal"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let chunk = rest
            .get(..size)
            .ok_or(HttpError::Malformed("a chunk shorter than its size"))?;
        body.extend_from_slice(chunk);
        rest = rest
            .get(size..)
            .and_then(|after| after.strip_prefix(b"\r\n"))
            .ok_or(HttpError::Malformed("a chunk not ended by CRLF"))?;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Answer {
    Page,
    Json,
    Fragment,
}

impl Answer {
    pub(crate) const ALL: [Self; 3] = [Self::Page, Self::Json, Self::Fragment];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Json => "json",
            Self::Fragment => "fragment",
        }
    }

    /// What decides the answer's shape in sofie-http's `Caller`.
    pub(crate) fn headers(self) -> Vec<(&'static str, String)> {
        match self {
            Self::Page => vec![("accept", "text/html".to_owned())],
            Self::Json => vec![("accept", "application/json".to_owned())],
            Self::Fragment => vec![
                ("accept", "text/html".to_owned()),
                ("hx-request", "true".to_owned()),
            ],
        }
    }
}

/// Cookies by session name; their values never leave `sofie mcp`.
#[derive(Default)]
pub(crate) struct CookieJars(HashMap<String, BTreeMap<String, String>>);

impl CookieJars {
    pub(crate) fn cookie_header(&self, session: &str) -> Option<String> {
        let jar = self.0.get(session).filter(|jar| !jar.is_empty())?;
        let pairs: Vec<String> = jar
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        Some(pairs.join("; "))
    }

    /// Answers the names of the cookies set or removed.
    pub(crate) fn store<'h>(
        &mut self,
        session: &str,
        set_cookies: impl Iterator<Item = &'h str>,
    ) -> Vec<String> {
        let jar = self.0.entry(session.to_owned()).or_default();
        let mut names = Vec::new();
        for set_cookie in set_cookies {
            let mut attributes = set_cookie.split(';');
            let Some((name, value)) = attributes.next().and_then(|pair| pair.split_once('='))
            else {
                continue;
            };
            let name = name.trim().to_owned();
            let removed = attributes.any(|attribute| {
                let attribute = attribute.trim().to_ascii_lowercase();
                attribute
                    .strip_prefix("max-age=")
                    .is_some_and(|seconds| seconds.parse::<i64>().is_ok_and(|seconds| seconds <= 0))
            });
            if removed {
                jar.remove(&name);
                names.push(format!("{name} (removed)"));
            } else {
                jar.insert(name.clone(), value.trim().to_owned());
                names.push(name);
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::{CookieJars, HttpError, Request, head, parse};

    #[test]
    fn a_chunked_answer_is_joined_and_its_headers_read_case_insensitively() {
        let answer = b"HTTP/1.1 303 See Other\r\nLocation: /orders/7\r\nSet-Cookie: a=1\r\n\
                       set-cookie: b=2; Path=/\r\nTransfer-Encoding: chunked\r\n\r\n\
                       5;ext=1\r\nhello\r\n7\r\n, world\r\n0\r\nTrailer: x\r\n\r\n";

        let response = parse(answer, false).unwrap();

        assert_eq!(response.status, 303);
        assert_eq!(response.reason, "See Other");
        assert_eq!(
            response.headers("location").collect::<Vec<_>>(),
            ["/orders/7"]
        );
        assert_eq!(
            response.headers("set-cookie").collect::<Vec<_>>(),
            ["a=1", "b=2; Path=/"]
        );
        assert_eq!(response.body, b"hello, world");
    }

    #[test]
    fn the_trace_id_is_read_from_the_app_s_own_traceparent() {
        let traced = b"HTTP/1.1 200 OK\r\ntraceparent: \
                       00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01\r\n\r\n";
        let untraced = b"HTTP/1.1 200 OK\r\n\r\n";

        assert_eq!(
            parse(traced, true).unwrap().trace_id(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(parse(untraced, true).unwrap().trace_id(), None);
    }

    #[test]
    fn a_body_is_cut_at_its_length_and_a_short_one_is_malformed() {
        let answer = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nokEXTRA";
        assert_eq!(parse(answer, false).unwrap().body, b"ok");
        assert_eq!(parse(answer, true).unwrap().body, b"");

        let short = b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\nok";
        assert!(matches!(parse(short, false), Err(HttpError::Malformed(_))));
        let broken = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n";
        assert!(matches!(parse(broken, false), Err(HttpError::Malformed(_))));
    }

    #[test]
    fn a_request_line_or_header_cannot_smuggle_another_line() {
        let address = "127.0.0.1:8080".parse().unwrap();
        let request = |method, path, value: &str| Request {
            method,
            path,
            headers: vec![("origin", value.to_owned())],
            body: b"",
        };

        let written = head(address, &request("POST", "/orders?a=1", "http://x")).unwrap();
        assert_eq!(
            written,
            "POST /orders?a=1 HTTP/1.1\r\nhost: 127.0.0.1:8080\r\nconnection: close\r\n\
             content-length: 0\r\norigin: http://x\r\n\r\n"
        );
        for (method, path, value) in [
            ("GET X", "/", "v"),
            ("GET", "/ HTTP/1.1\r\nx: y", "v"),
            ("GET", "orders", "v"),
            ("GET", "/", "v\r\nx: y"),
        ] {
            assert!(
                head(address, &request(method, path, value)).is_err(),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_jar_keeps_cookies_per_session_and_forgets_a_removed_one() {
        let mut jars = CookieJars::default();

        let set = jars.store(
            "alice",
            [
                "__Host-shop-session=sealed; Max-Age=3600; Path=/",
                "theme=dark",
            ]
            .into_iter(),
        );
        assert_eq!(set, ["__Host-shop-session", "theme"]);
        assert_eq!(
            jars.cookie_header("alice").as_deref(),
            Some("__Host-shop-session=sealed; theme=dark")
        );
        assert_eq!(jars.cookie_header("bob"), None);

        let removed = jars.store(
            "alice",
            ["__Host-shop-session=; Max-Age=0; Path=/"].into_iter(),
        );
        assert_eq!(removed, ["__Host-shop-session (removed)"]);
        assert_eq!(jars.cookie_header("alice").as_deref(), Some("theme=dark"));
    }
}

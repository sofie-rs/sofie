use std::ops::ControlFlow;
use std::sync::Arc;

use http::header::{REFERER, SET_COOKIE};
use http::request::Parts;
use http::{HeaderValue, Method, StatusCode, Uri};
use hyper::body::Incoming;
use serde_json::{Map, Value};
use sofie_core::{Error, Instant, Result, Uuid};
use sofie_db::Db;

use super::Toolbar;
use super::overrides::{ActingAs, Overrides, token};
use crate::answer::{self, Redirect, Response};
use crate::csrf;
use crate::pipeline::{Asked, read_body};

pub(super) const PATH: &str = "/_sofie/toolbar";
const FORM_LIMIT: usize = 4 * 1024;
const SECONDS_PER_HOUR: i64 = 3600;

impl Toolbar {
    pub(crate) async fn answer(
        &self,
        asked: Asked<'_>,
        db: &Db,
        body: &mut Incoming,
    ) -> ControlFlow<Response> {
        if asked.request.uri.path() != PATH {
            return ControlFlow::Continue(());
        }
        ControlFlow::Break(self.respond(asked, db, body).await)
    }

    async fn respond(&self, asked: Asked<'_>, db: &Db, body: &mut Incoming) -> Response {
        let (request, caller) = (asked.request, asked.caller);
        if request.method != Method::POST {
            return answer::method_not_allowed(&["POST"], caller);
        }
        if !asked.client.to_canonical().is_loopback() {
            return answer::refusal(
                StatusCode::FORBIDDEN,
                "The toolbar takes forms from this machine only.",
                caller,
            );
        }
        if csrf::refused_by(request, &self.cookies).is_some() {
            return answer::refusal(
                StatusCode::FORBIDDEN,
                "The toolbar takes forms from the app's own pages only.",
                caller,
            );
        }
        let body = match read_body(body, FORM_LIMIT, caller).await {
            Ok(body) => body,
            Err(refusal) => return *refusal,
        };
        let overrides = Overrides::of(&request.headers, self.cookies.toolbar());
        crate::form::parse(&body)
            .and_then(|form| self.changed(overrides, &form, db))
            .and_then(|overrides| back(request, overrides.cookie(self.cookies.toolbar())?))
            .unwrap_or_else(|error| answer::failure(error, caller))
    }

    fn changed(
        &self,
        mut overrides: Overrides,
        form: &Map<String, Value>,
        db: &Db,
    ) -> Result<Overrides> {
        let field = |name: &str| {
            form.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::BadRequest(format!("the toolbar's form has no `{name}`")))
        };
        match field("action")? {
            "act_as" => {
                overrides.acting_as = Some(self.acting_as(field("area")?, field("principal")?)?);
            }
            "stop_acting" => overrides.acting_as = None,
            "flag" => {
                overrides.flags = with_flag(&overrides.flags, field("flag")?, field("answer")?)?;
            }
            "clear_flags" => overrides.flags = Arc::new([]),
            "set_clock" => overrides.clock_seconds = Some(seconds_until(field("at")?, db)?),
            "shift_clock" => {
                let shifted = shifted_seconds(overrides.clock_seconds, field("hours")?)?;
                overrides.clock_seconds = Some(shifted);
            }
            "reset_clock" => overrides.clock_seconds = None,
            other => {
                return Err(Error::BadRequest(format!(
                    "the toolbar has no action `{other}`"
                )));
            }
        }
        Ok(overrides)
    }

    /// Whoever the id names: in development any principal may be tried, member or not.
    fn acting_as(&self, area: &str, principal: &str) -> Result<ActingAs> {
        let area = self
            .areas
            .iter()
            .copied()
            .find(|known| *known == area)
            .ok_or_else(|| Error::BadRequest(format!("the app has no signed-in area `{area}`")))?;
        let principal: Uuid = principal
            .trim()
            .parse()
            .map_err(|_| Error::BadRequest(format!("`{principal}` is not a principal's id")))?;
        Ok(ActingAs {
            area: area.to_owned(),
            principal,
        })
    }
}

fn with_flag(
    flags: &[(String, String)],
    name: &str,
    answer: &str,
) -> Result<Arc<[(String, String)]>> {
    let (name, answer) = (token(name.trim())?, token(answer.trim())?);
    let mut changed: Vec<(String, String)> = flags
        .iter()
        .filter(|(overridden, _)| *overridden != name)
        .cloned()
        .collect();
    changed.push((name, answer));
    Ok(changed.into())
}

/// From the app's own clock, which a request's clock is shifted from.
fn seconds_until(at: &str, db: &Db) -> Result<i64> {
    let at = Instant::parse(at.trim())?;
    Ok(at
        .timestamp()
        .duration_since(db.clock().now().timestamp())
        .as_secs())
}

fn shifted_seconds(current: Option<i64>, hours: &str) -> Result<i64> {
    let hours: i64 = hours
        .trim()
        .parse()
        .map_err(|_| Error::BadRequest(format!("`{hours}` is not a whole number of hours")))?;
    hours
        .checked_mul(SECONDS_PER_HOUR)
        .and_then(|seconds| seconds.checked_add(current.unwrap_or(0)))
        .ok_or_else(|| Error::BadRequest(format!("{hours} hours is too far to shift the clock")))
}

/// Only the referring page's path is kept, so the answer never leads off the app.
fn back(request: &Parts, cookie: HeaderValue) -> Result<Response> {
    let location = request
        .headers
        .get(REFERER)
        .and_then(|referer| referer.to_str().ok())
        .and_then(|referer| referer.parse::<Uri>().ok())
        .and_then(|referer| referer.path_and_query().map(ToString::to_string))
        .filter(|path| crate::guard::is_local_path(path))
        .unwrap_or_else(|| "/".to_owned());
    let mut response = answer::redirect(Redirect::SeeOther, &location)?;
    response.headers_mut().append(SET_COOKIE, cookie);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shift_adds_whole_hours_to_the_one_held() {
        assert_eq!(shifted_seconds(None, "25").unwrap(), 90_000);
        assert_eq!(shifted_seconds(Some(3600), " -2 ").unwrap(), -3600);
        assert!(shifted_seconds(None, "1.5").is_err());
        assert!(shifted_seconds(None, &i64::MAX.to_string()).is_err());
    }

    #[test]
    fn a_flag_answered_again_replaces_its_override() {
        let flags = with_flag(&[], "shop.beta", "on").unwrap();

        let flags = with_flag(&flags, "shop.beta", "off").unwrap();

        assert_eq!(&*flags, [("shop.beta".to_owned(), "off".to_owned())]);
        assert!(with_flag(&flags, "shop beta", "on").is_err());
    }
}

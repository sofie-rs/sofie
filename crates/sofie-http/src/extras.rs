use bytes::Bytes;
use http::HeaderMap;
use http::header::CONTENT_TYPE;
use http_body_util::{BodyExt, Full};

use crate::answer::{HTML, Response};
use crate::health::Endpoints;
use crate::trace::RequestTrace;

const DOCTYPE: &str = "<!doctype html>";

impl Endpoints {
    #[cfg_attr(
        not(all(feature = "browser-telemetry", feature = "toolbar", debug_assertions)),
        expect(
            unused_variables,
            reason = "browser telemetry reads the route, the toolbar the request's headers"
        )
    )]
    pub(crate) async fn with_extras(
        &self,
        response: Response,
        trace: &RequestTrace,
        route: Option<&'static str>,
        request: &HeaderMap,
    ) -> Response {
        let is_html = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|content_type| content_type == HTML);
        if !is_html {
            return response;
        }
        let (parts, body) = response.into_parts();
        let Ok(collected) = body.collect().await;
        let bytes = collected.to_bytes();
        if !bytes.starts_with(DOCTYPE.as_bytes()) {
            return Response::from_parts(parts, Full::new(bytes));
        }
        // A sole owner hands its buffer over, so the page is not copied.
        let mut page = match String::from_utf8(Vec::from(bytes)) {
            Ok(page) => page,
            Err(error) => return Response::from_parts(parts, Full::new(error.into_bytes().into())),
        };
        #[cfg(all(feature = "development", debug_assertions))]
        if let Some(script) = crate::development::client_script(&parts.headers) {
            insert_before(&mut page, "</head>", &script);
        }
        #[cfg(feature = "browser-telemetry")]
        {
            let traceparent = trace
                .span()
                .in_scope(sofie_telemetry::current_traceparent)
                .map(|traceparent| traceparent.to_string())
                .unwrap_or_default();
            insert_before(
                &mut page,
                "</head>",
                &self.browser.meta(route, &traceparent),
            );
        }
        #[cfg(all(feature = "toolbar", debug_assertions))]
        insert_before(
            &mut page,
            "</body>",
            &self.toolbar.html(&trace.span(), request),
        );
        Response::from_parts(parts, Full::new(Bytes::from(page)))
    }
}

fn insert_before(page: &mut String, closing: &str, added: &str) {
    if let Some(at) = page.rfind(closing) {
        page.insert_str(at, added);
    }
}

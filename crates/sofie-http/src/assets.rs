use http::header::CACHE_CONTROL;
use http::{HeaderValue, StatusCode};
use sofie_engine::Asset;

use crate::answer::{self, Response};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Served for good at content-hashed paths.
pub(crate) struct Assets {
    pub(crate) served: Vec<(String, Asset)>,
}

impl Assets {
    pub(crate) fn embedded() -> Self {
        let assets = [
            sofie_engine::CLIENT_SCRIPT,
            sofie_engine::ENGINE_SCRIPT,
            sofie_engine::STYLESHEET,
        ];
        Self {
            served: assets.map(|asset| (asset.path(), asset)).into(),
        }
    }

    pub(crate) fn response(&self, path: &str) -> Option<Response> {
        let (_, asset) = self.served.iter().find(|(served, _)| served == path)?;
        let mut response = answer::response(StatusCode::OK, Some(asset.content_type), asset.bytes);
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
        #[cfg(feature = "compression")]
        if let Some(precompressed) = asset.precompressed() {
            response.extensions_mut().insert(precompressed);
        }
        Some(response)
    }
}

#[cfg(test)]
mod tests {
    use http::header::CONTENT_TYPE;

    use super::*;

    #[test]
    fn assets_are_served_for_good_at_their_paths() {
        let assets = Assets::embedded();

        for (asset, content_type) in [
            (
                sofie_engine::CLIENT_SCRIPT,
                "application/javascript; charset=utf-8",
            ),
            (
                sofie_engine::ENGINE_SCRIPT,
                "application/javascript; charset=utf-8",
            ),
            (sofie_engine::STYLESHEET, "text/css; charset=utf-8"),
        ] {
            let response = assets.response(&asset.path()).unwrap();
            assert_eq!(response.headers()[CACHE_CONTROL], IMMUTABLE);
            assert_eq!(response.headers()[CONTENT_TYPE], content_type);
        }
        assert!(assets.response("/_sofie/htmx.min.js").is_none());
    }
}

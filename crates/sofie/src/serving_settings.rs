use sofie_core::{Error, Result};
use sofie_http::Serving;

#[cfg(feature = "trusted-proxies")]
use crate::settings::{REQUEST_ID_HEADER, TRUSTED_PROXIES};
#[cfg(feature = "metrics")]
const METRICS_TOKEN: &str = "SOFIE_METRICS_TOKEN";
#[cfg(feature = "browser-telemetry")]
const BROWSER_SAMPLE: &str = "SOFIE_BROWSER_SAMPLE";
#[cfg(feature = "cors")]
const CORS_ORIGINS: &str = "SOFIE_CORS_ORIGINS";
#[cfg(feature = "tls")]
const TLS_CERTIFICATE: &str = "SOFIE_TLS_CERTIFICATE";
#[cfg(feature = "tls")]
const TLS_KEY: &str = "SOFIE_TLS_KEY";
#[cfg(feature = "shield")]
const SHIELD: &str = "SOFIE_SHIELD";
#[cfg(feature = "shield")]
const SHIELD_THRESHOLDS: &str = "SOFIE_SHIELD_THRESHOLDS";
#[cfg(feature = "shield")]
const SHIELD_SHARED: &str = "SOFIE_SHIELD_SHARED";

pub(crate) fn from_environment() -> Result<impl FnOnce(Serving) -> Serving> {
    configured(sofie_core::environment_variable)
}

/// The token is never part of an error: a log must not carry it.
fn configured(
    lookup: impl Fn(&str) -> Result<Option<String>>,
) -> Result<impl FnOnce(Serving) -> Serving> {
    #[cfg(feature = "trusted-proxies")]
    let proxies = trusted_proxies(&lookup)?;
    #[cfg(feature = "metrics")]
    let token = lookup(METRICS_TOKEN)?
        .map(|token| sofie_http::MetricsToken::parse(&token))
        .transpose()
        .map_err(|error| Error::internal(METRICS_TOKEN, error))?;
    #[cfg(feature = "browser-telemetry")]
    let sample = lookup(BROWSER_SAMPLE)?
        .map(|share| sofie_http::BrowserSample::parse(&share))
        .transpose()
        .map_err(|error| Error::internal(BROWSER_SAMPLE, error))?;
    #[cfg(feature = "shield")]
    let shield = shield(&lookup)?;
    #[cfg(feature = "cors")]
    let cors_origins = sofie_http::CorsOrigins::parse(&lookup(CORS_ORIGINS)?.unwrap_or_default())
        .map_err(|error| Error::internal(CORS_ORIGINS, error))?;
    #[cfg(feature = "storage")]
    let storage = crate::storage_settings::configured(&lookup)?;
    #[cfg(feature = "tls")]
    let tls = tls(&lookup)?;
    Ok(move |serving: Serving| {
        #[cfg(feature = "trusted-proxies")]
        let serving = serving.trusted_proxies(proxies);
        #[cfg(feature = "metrics")]
        let serving = match token {
            Some(token) => serving.metrics_token(token),
            None => serving,
        };
        #[cfg(feature = "browser-telemetry")]
        let serving = match sample {
            Some(sample) => serving.browser_sample(sample),
            None => serving,
        };
        #[cfg(feature = "shield")]
        let serving = serving.shield(shield);
        #[cfg(feature = "cors")]
        let serving = serving.cors_origins(cors_origins);
        #[cfg(feature = "storage")]
        let serving = serving.storage(storage);
        #[cfg(feature = "tls")]
        let serving = match tls {
            Some(tls) => serving.tls(tls),
            None => serving,
        };
        serving
    })
}

/// Both files or neither: half a pair would leave the app on plain HTTP unnoticed.
#[cfg(feature = "tls")]
fn tls(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<Option<sofie_http::Tls>> {
    match (lookup(TLS_CERTIFICATE)?, lookup(TLS_KEY)?) {
        (None, None) => Ok(None),
        (Some(certificate), Some(key)) => sofie_http::Tls::from_pem_files(
            std::path::Path::new(&certificate),
            std::path::Path::new(&key),
        )
        .map(Some),
        (Some(_), None) => Err(Error::internal_message(format!(
            "{TLS_CERTIFICATE} is set without {TLS_KEY}; set both to serve HTTPS, or neither"
        ))),
        (None, Some(_)) => Err(Error::internal_message(format!(
            "{TLS_KEY} is set without {TLS_CERTIFICATE}; set both to serve HTTPS, or neither"
        ))),
    }
}

#[cfg(feature = "trusted-proxies")]
fn trusted_proxies(
    lookup: &impl Fn(&str) -> Result<Option<String>>,
) -> Result<sofie_http::TrustedProxies> {
    let proxies = sofie_http::TrustedProxies::parse(&lookup(TRUSTED_PROXIES)?.unwrap_or_default())
        .map_err(|error| Error::internal(TRUSTED_PROXIES, error))?;
    match lookup(REQUEST_ID_HEADER)? {
        Some(name) => proxies
            .request_id_header(&name)
            .map_err(|error| Error::internal(REQUEST_ID_HEADER, error)),
        None => Ok(proxies),
    }
}

/// `SOFIE_SHIELD` defaults to dry-run in a debug build and on in a release one.
#[cfg(feature = "shield")]
fn shield(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<sofie_http::Shield> {
    let mut shield = match lookup(SHIELD)? {
        Some(mode) => sofie_http::Shield::new(
            sofie_http::ShieldMode::parse(&mode).map_err(|error| Error::internal(SHIELD, error))?,
        ),
        None => sofie_http::Shield::default(),
    };
    if let Some(thresholds) = lookup(SHIELD_THRESHOLDS)? {
        let thresholds = sofie_http::Thresholds::parse(&thresholds)
            .map_err(|error| Error::internal(SHIELD_THRESHOLDS, error))?;
        shield = shield.thresholds(thresholds);
    }
    if crate::settings::switch(SHIELD_SHARED, lookup(SHIELD_SHARED)?.as_deref())? {
        shield = shield.shared();
    }
    Ok(shield)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<impl FnOnce(Serving) -> Serving> {
        configured(|name| {
            Ok(crate::settings::given(variables, name).or_else(|| storage_default(name)))
        })
    }

    /// A store needs keys and a root, which these tests are not about.
    fn storage_default(name: &str) -> Option<String> {
        if !cfg!(feature = "storage") {
            return None;
        }
        match name {
            "SOFIE_SIGNING_KEYS" => Some(format!("{}=", "A".repeat(43))),
            "SOFIE_STORAGE_ROOT" => Some(
                std::env::temp_dir()
                    .join("sofie-serving-settings")
                    .display()
                    .to_string(),
            ),
            _ => None,
        }
    }

    #[cfg(feature = "trusted-proxies")]
    #[test]
    fn trusted_proxies_are_read_and_a_malformed_one_refused() {
        assert!(read(&[]).is_ok());
        assert!(read(&[(TRUSTED_PROXIES, "10.0.0.0/8, 192.0.2.7")]).is_ok());

        let error = read(&[(TRUSTED_PROXIES, "10.0.0.0/40")]).err().unwrap();

        assert!(error.to_string().starts_with(TRUSTED_PROXIES), "{error}");
    }

    #[cfg(feature = "trusted-proxies")]
    #[test]
    fn a_request_id_header_is_read_and_a_credential_or_malformed_one_refused() {
        assert!(read(&[(REQUEST_ID_HEADER, "fly-request-id")]).is_ok());

        for name in ["not a header", "Authorization"] {
            let error = read(&[(REQUEST_ID_HEADER, name)]).err().unwrap();
            assert!(error.to_string().starts_with(REQUEST_ID_HEADER), "{error}");
        }
    }

    #[cfg(feature = "shield")]
    #[test]
    fn the_shield_is_read_and_a_malformed_setting_refused_naming_its_key() {
        assert!(read(&[]).is_ok());
        assert!(
            read(&[
                (SHIELD, "dry-run"),
                (SHIELD_THRESHOLDS, "scan=60, web.sign_in=5"),
                (SHIELD_SHARED, "on"),
            ])
            .is_ok()
        );

        for (key, value) in [
            (SHIELD, "loud"),
            (SHIELD_THRESHOLDS, "scan=lots"),
            (SHIELD_SHARED, "yes"),
        ] {
            let error = read(&[(key, value)]).err().unwrap();
            assert!(error.to_string().starts_with(key), "{error}");
        }
    }

    #[cfg(feature = "cors")]
    #[test]
    fn cors_origins_are_read_and_a_malformed_one_refused_naming_its_key() {
        assert!(read(&[(CORS_ORIGINS, "api=https://app.example")]).is_ok());

        let error = read(&[(CORS_ORIGINS, "api=app.example")]).err().unwrap();

        assert!(error.to_string().starts_with(CORS_ORIGINS), "{error}");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn half_a_tls_pair_is_refused() {
        let error = read(&[(TLS_CERTIFICATE, "/etc/app/cert.pem")])
            .err()
            .expect("a certificate without its key is refused")
            .to_string();

        assert!(error.contains(TLS_KEY), "{error}");
        assert!(read(&[]).is_ok());
    }

    #[cfg(feature = "metrics")]
    #[test]
    fn a_metrics_token_is_read_and_a_short_one_refused_without_showing_it() {
        assert!(read(&[(METRICS_TOKEN, &"a".repeat(40))]).is_ok());

        let error = read(&[(METRICS_TOKEN, "short-secret")]).err().unwrap();

        assert!(error.to_string().starts_with(METRICS_TOKEN), "{error}");
        assert!(!format!("{error:?}").contains("short-secret"), "{error:?}");
    }
}

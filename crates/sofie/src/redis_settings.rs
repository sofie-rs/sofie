use sofie_core::{Error, Result};

const REDIS_URL: &str = "SOFIE_REDIS_URL";

/// Connected once at start and shared by every feature that uses Redis; `None` when unset.
#[cfg(feature = "redis")]
pub(crate) async fn connected(
    lookup: &impl Fn(&str) -> Result<Option<String>>,
) -> Result<Option<sofie_redis::Redis>> {
    let Some(url) = lookup(REDIS_URL)? else {
        return Ok(None);
    };
    let url =
        sofie_redis::RedisUrl::parse(&url).map_err(|error| Error::internal(REDIS_URL, error))?;
    sofie_redis::Redis::connect(url).await.map(Some)
}

/// A URL the build cannot use would otherwise be ignored without a word.
#[cfg(not(feature = "redis"))]
pub(crate) fn refuse_unbuilt(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<()> {
    if lookup(REDIS_URL)?.is_none() {
        return Ok(());
    }
    Err(Error::internal_message(format!(
        "{REDIS_URL} is set, but the app is built without Redis; enable sofie's `redis` feature"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(
        variables: &[(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Result<Option<String>> {
        move |name| Ok(crate::settings::given(variables, name))
    }

    #[cfg(feature = "redis")]
    #[tokio::test]
    async fn no_url_connects_nothing_and_a_malformed_one_is_refused_without_its_password() {
        assert!(connected(&lookup(&[])).await.unwrap().is_none());

        let malformed = [(REDIS_URL, "http://user:hunter2@host")];
        let error = connected(&lookup(&malformed))
            .await
            .err()
            .unwrap()
            .to_string();

        assert!(
            error.starts_with(REDIS_URL) && !error.contains("hunter2"),
            "{error}"
        );
    }

    #[cfg(not(feature = "redis"))]
    #[test]
    fn a_url_without_the_feature_is_refused() {
        assert!(refuse_unbuilt(&lookup(&[])).is_ok());

        let error = refuse_unbuilt(&lookup(&[(REDIS_URL, "redis://cache")]))
            .unwrap_err()
            .to_string();

        assert!(error.contains("`redis` feature"), "{error}");
    }
}

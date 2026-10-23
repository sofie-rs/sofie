use std::sync::Arc;

use sofie_core::{Error, Result};
use sofie_http::SigningKeys;
use sofie_storage::{Local, Storage};

const STORAGE: &str = "SOFIE_STORAGE";
const ROOT: &str = "SOFIE_STORAGE_ROOT";
const DEFAULT_ROOT: &str = "storage";
#[cfg(feature = "storage-s3")]
const CHOICES: &str = "local, s3";
#[cfg(not(feature = "storage-s3"))]
const CHOICES: &str = "local (s3 needs the `storage-s3` feature)";

pub(crate) fn configured(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<Storage> {
    match lookup(STORAGE)?.as_deref().unwrap_or("local") {
        "local" => {
            // The keys sofie-http checks a `Local` store's links with.
            let keys = SigningKeys::read(lookup)?;
            let root = lookup(ROOT)?.unwrap_or_else(|| DEFAULT_ROOT.to_owned());
            let local = Local::new(root, Arc::new(keys))?;
            Ok(Storage::new(local))
        }
        #[cfg(feature = "storage-s3")]
        "s3" => s3::configured(lookup),
        other => Err(Error::internal_message(format!(
            "{STORAGE} is `{other}`, not one of {CHOICES}"
        ))),
    }
}

#[cfg(feature = "storage-s3")]
mod s3 {
    use sofie_core::{Error, Result};
    use sofie_storage::{S3, S3Settings, Storage};

    const ENDPOINT: &str = "SOFIE_STORAGE_S3_ENDPOINT";
    const REGION: &str = "SOFIE_STORAGE_S3_REGION";
    const BUCKET: &str = "SOFIE_STORAGE_S3_BUCKET";
    const ACCESS_KEY_ID: &str = "SOFIE_STORAGE_S3_ACCESS_KEY_ID";
    const SECRET_ACCESS_KEY: &str = "SOFIE_STORAGE_S3_SECRET_ACCESS_KEY";
    const PATH_STYLE: &str = "SOFIE_STORAGE_S3_PATH_STYLE";
    /// A service inside the network: private addresses and plain HTTP allowed.
    const PRIVATE: &str = "SOFIE_STORAGE_S3_PRIVATE";

    pub(super) fn configured(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<Storage> {
        let required = |name: &str| {
            lookup(name)?.ok_or_else(|| Error::internal_message(format!("{name} is not set")))
        };
        let settings = S3Settings {
            endpoint: lookup(ENDPOINT)?,
            region: required(REGION)?,
            bucket: required(BUCKET)?,
            access_key_id: required(ACCESS_KEY_ID)?,
            secret_access_key: required(SECRET_ACCESS_KEY)?,
            path_style: crate::settings::switch(PATH_STYLE, lookup(PATH_STYLE)?.as_deref())?,
        };
        let private = crate::settings::switch(PRIVATE, lookup(PRIVATE)?.as_deref())?;
        let http = sofie_client::for_vendor("s3", settings.vendor(private)?)?;
        Ok(Storage::new(S3::new(http, settings)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Storage> {
        configured(&|name: &str| Ok(crate::settings::given(variables, name)))
    }

    const KEYS: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    #[test]
    fn a_local_store_needs_the_signing_keys_and_an_unknown_store_is_named() {
        let root = std::env::temp_dir().join(format!(
            "sofie-storage-settings-{}",
            sofie_storage::StorageKey::generate().unwrap()
        ));
        let root_text = root.to_str().unwrap();

        assert!(read(&[(sofie_auth::SIGNING_KEYS, KEYS), (ROOT, root_text)]).is_ok());
        assert!(root.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
        let missing = read(&[(ROOT, root_text)]).err().unwrap().to_string();
        assert!(missing.contains(sofie_auth::SIGNING_KEYS), "{missing}");
        let unknown = read(&[(STORAGE, "ftp")]).err().unwrap().to_string();
        assert!(unknown.starts_with("SOFIE_STORAGE is `ftp`"), "{unknown}");
    }
}

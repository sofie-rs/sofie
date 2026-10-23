use sofie_auth::SigningKeys;
use sofie_core::Result;

/// Where a bulk mail's unsubscribe link goes, below the app's public URL.
pub const PATH: &str = "/mail/unsubscribe";
pub(crate) const PURPOSE: &str = "sofie_mail.unsubscribe";

/// The address and list, sealed: the link names neither, and nobody forges one.
pub(crate) fn token(keys: &SigningKeys, address: &str, list: &str) -> Result<String> {
    keys.seal_value(PURPOSE, format!("{address}\n{list}").as_bytes())
}

#[cfg(feature = "web")]
pub(crate) fn opened(plain: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(plain).ok()?;
    let (address, list) = text.split_once('\n')?;
    Some((address.to_owned(), list.to_owned()))
}

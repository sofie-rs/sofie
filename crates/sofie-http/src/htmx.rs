use http::HeaderName;

pub(crate) const REQUEST: HeaderName = HeaderName::from_static("hx-request");
pub(crate) const TARGET: HeaderName = HeaderName::from_static("hx-target");
pub(crate) const TRIGGER: HeaderName = HeaderName::from_static("hx-trigger");
pub(crate) const REDIRECT: HeaderName = HeaderName::from_static("hx-redirect");
#[cfg(feature = "engine")]
pub(crate) const RETARGET: HeaderName = HeaderName::from_static("hx-retarget");
#[cfg(feature = "engine")]
pub(crate) const RESWAP: HeaderName = HeaderName::from_static("hx-reswap");

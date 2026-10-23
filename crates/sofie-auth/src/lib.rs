//! Signing and sealing: sessions sealed into a cookie with no server store, signed links and tokens.

mod bearer;
mod digest;
mod principal_token;
mod session;
mod signing;

pub use bearer::BearerToken;
pub use digest::digest;
pub use principal_token::{Acting, TokenRefused};
pub use session::{OLD_SIGNING_KEYS, OpenError, SIGNING_KEYS, SessionClaims, SigningKeys};
pub use signing::Signed;

use ring::rand::{SecureRandom, SystemRandom};
use sofie_core::{Error, Result};

fn random_bytes<const N: usize>(purpose: &str) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        Error::internal_message(format!("the system gave no random bytes for {purpose}"))
    })?;
    Ok(bytes)
}

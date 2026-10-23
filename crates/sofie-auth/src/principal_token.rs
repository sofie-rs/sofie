use std::fmt;

use sofie_core::{Timestamp, Uuid};

use crate::{Signed, SigningKeys};

/// Long enough for one call between apps, short enough that a leaked token soon expires.
const LIFETIME_SECONDS: i64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Acting {
    pub area: String,
    pub principal: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenRefused {
    Malformed,
    Signature,
    Expired,
}

impl fmt::Display for TokenRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "the principal token is malformed",
            Self::Signature => "the principal token's signature does not verify",
            Self::Expired => "the principal token has expired",
        })
    }
}

impl std::error::Error for TokenRefused {}

impl SigningKeys {
    /// Valid only for `target`, the path it is sent to, so it cannot be replayed against another.
    #[must_use]
    pub fn principal_token(&self, target: &str, acting: Option<&Acting>, now: Timestamp) -> String {
        let expires = now.as_second() + LIFETIME_SECONDS;
        let payload = match acting {
            Some(acting) => format!("{expires}:{}:{}", acting.area, acting.principal),
            None => format!("{expires}::"),
        };
        let signature = self.sign(Signed::PrincipalToken, signed(target, &payload).as_bytes());
        format!("{payload}.{signature}")
    }

    pub fn verify_principal_token(
        &self,
        token: &str,
        target: &str,
        now: Timestamp,
    ) -> std::result::Result<Option<Acting>, TokenRefused> {
        let (payload, signature) = token.rsplit_once('.').ok_or(TokenRefused::Malformed)?;
        if !self.verifies(
            Signed::PrincipalToken,
            signed(target, payload).as_bytes(),
            signature,
        ) {
            return Err(TokenRefused::Signature);
        }
        let mut parts = payload.splitn(3, ':');
        let expires: i64 = parts
            .next()
            .and_then(|expires| expires.parse().ok())
            .ok_or(TokenRefused::Malformed)?;
        // A token living past its lifetime came from an issuer whose clock is wrong.
        let now = now.as_second();
        if now >= expires || expires > now + 2 * LIFETIME_SECONDS {
            return Err(TokenRefused::Expired);
        }
        match (parts.next(), parts.next()) {
            (Some(""), Some("")) => Ok(None),
            (Some(area), Some(principal)) => Ok(Some(Acting {
                area: area.to_owned(),
                principal: principal.parse().map_err(|_| TokenRefused::Malformed)?,
            })),
            _ => Err(TokenRefused::Malformed),
        }
    }
}

fn signed(target: &str, payload: &str) -> String {
    format!("{target}\n{payload}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::keys;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn customer() -> Acting {
        Acting {
            area: "customer".to_owned(),
            principal: Uuid::now_v7().unwrap(),
        }
    }

    #[test]
    fn a_token_carries_its_principal_to_its_target_until_it_expires() {
        let acting = customer();
        let token = keys(1).principal_token("/_sofie/contracts/a/B/c", Some(&acting), at(1000));
        let verify = |target: &str, keys: SigningKeys, now: i64| {
            keys.verify_principal_token(&token, target, at(now))
        };

        assert_eq!(
            verify("/_sofie/contracts/a/B/c", keys(1), 1059),
            Ok(Some(acting))
        );
        assert_eq!(
            verify("/_sofie/contracts/a/B/d", keys(1), 1001),
            Err(TokenRefused::Signature)
        );
        assert_eq!(
            verify("/_sofie/contracts/a/B/c", keys(2), 1001),
            Err(TokenRefused::Signature)
        );
        assert_eq!(
            verify("/_sofie/contracts/a/B/c", keys(1), 1060),
            Err(TokenRefused::Expired)
        );
        assert_eq!(
            verify("/_sofie/contracts/a/B/c", keys(1), 800),
            Err(TokenRefused::Expired)
        );
    }

    #[test]
    fn an_app_s_own_token_acts_for_nobody_and_a_forgery_is_refused() {
        let token = keys(1).principal_token("/_sofie/feed", None, at(0));

        assert_eq!(
            keys(1).verify_principal_token(&token, "/_sofie/feed", at(1)),
            Ok(None)
        );
        assert_eq!(
            keys(1).verify_principal_token("no-dot", "/_sofie/feed", at(1)),
            Err(TokenRefused::Malformed)
        );
        let link = keys(1).sign(Signed::Link, b"/_sofie/feed");
        assert_eq!(
            keys(1).verify_principal_token(&format!("x.{link}"), "/_sofie/feed", at(1)),
            Err(TokenRefused::Signature)
        );
    }

    #[test]
    fn a_token_is_its_plain_payload_and_a_signature() {
        let acting = customer();

        let token = keys(1).principal_token("/_sofie/feed", Some(&acting), at(1000));

        let prefix = format!("1060:customer:{}.", acting.principal);
        assert!(token.starts_with(&prefix), "{token}");
    }
}

//! Without the `tls` feature, `disable` and `prefer` connect in plain text, as libpq does when it
//! has no TLS, and a url asking for encryption is refused rather than sent in the clear.

use sofie_core::{Error, Result};
use tokio_postgres::config::SslMode;
use tokio_postgres::tls::{NoTls, NoTlsStream};

pub(crate) type Tls = NoTls;
pub(crate) type Stream = NoTlsStream;

const ENCRYPTING: [&str; 4] = [
    "sslmode=require",
    "sslmode=verify-ca",
    "sslmode=verify-full",
    "sslrootcert",
];

pub(crate) fn configure(url: &str) -> Result<(String, NoTls)> {
    let asked = ENCRYPTING
        .into_iter()
        .find(|option| url.contains(option))
        .or_else(|| requires_tls(url).then_some("sslmode=require"));
    match asked {
        Some(option) => Err(Error::internal_message(format!(
            "DATABASE_URL asks for {option}, but this build has no Postgres TLS; enable sofie's \
             `postgres-tls` feature"
        ))),
        None => Ok((url.to_owned(), NoTls)),
    }
}

/// The driver also reads a quoted `sslmode='require'` in the keyword form.
fn requires_tls(url: &str) -> bool {
    url.parse::<tokio_postgres::Config>()
        .is_ok_and(|config| matches!(config.get_ssl_mode(), SslMode::Require))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_url_asking_for_encryption_is_refused_and_the_plain_modes_are_accepted() {
        for asked in [
            "sslmode=require",
            "sslmode=verify-ca",
            "sslmode=verify-full",
            "sslmode=verify-full&sslrootcert=/etc/ca.pem",
        ] {
            let url = format!("postgres://nobody@localhost/nothing?{asked}");

            let refused = crate::Postgres::connect(&url).err().unwrap().to_string();

            assert!(
                refused.contains("enable sofie's `postgres-tls`"),
                "{refused}"
            );
        }
        let refused = crate::Postgres::connect("host=localhost sslmode='require'")
            .err()
            .unwrap()
            .to_string();
        assert!(refused.contains("asks for sslmode=require"), "{refused}");
        for accepted in [
            "postgres://nobody@localhost/nothing",
            "postgres://nobody@localhost/nothing?sslmode=prefer",
            "postgres://nobody@localhost/nothing?sslmode=disable",
        ] {
            assert!(crate::Postgres::connect(accepted).is_ok(), "{accepted}");
        }
    }
}

use std::net::IpAddr;

use crate::encoding::percent;

/// What drops or fills a database unasked runs only on one on this machine. Read as
/// tokio-postgres reads the url, so the guard and the driver agree on where it connects.
#[must_use]
pub fn is_local_database(url: &str) -> bool {
    if url.starts_with("sqlite://") || url == "sqlite::memory:" {
        return true;
    }
    let Some(rest) = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
    else {
        return false;
    };
    // The driver ends the credentials at the first `@`, even one in the path or a parameter.
    let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (hosts, path) = rest.split_at(end);
    let parameters = path.split_once('?').map_or("", |(_, query)| query);
    let Some(parameter_hosts) = parameter_hosts(parameters) else {
        return false;
    };
    hosts
        .split(',')
        .map(without_port)
        .chain(parameter_hosts.iter().map(String::as_str))
        .all(is_local_host)
}

/// `None` when a parameter points elsewhere. The driver decodes keys and values, so a name
/// written as `h%6Fst` still counts; a host list is split even where unix reads one socket path.
fn parameter_hosts(parameters: &str) -> Option<Vec<String>> {
    let mut hosts = Vec::new();
    for parameter in parameters.split('&') {
        let Some((key, value)) = parameter.split_once('=') else {
            continue;
        };
        match (decoded(key)?.as_str(), decoded(value)?) {
            ("host", value) => hosts.extend(value.split(',').map(str::to_owned)),
            ("hostaddr", addresses) if !addresses.split(',').all(is_loopback) => return None,
            // Names a connection defined elsewhere, which may be anywhere.
            ("service", _) => return None,
            _ => {}
        }
    }
    Some(hosts)
}

fn decoded(text: &str) -> Option<String> {
    String::from_utf8(percent::decode(text.as_bytes())?).ok()
}

fn is_loopback(address: &str) -> bool {
    address
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn without_port(host: &str) -> &str {
    if let Some(bracketed) = host.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or_default();
    }
    host.split(':').next().unwrap_or_default()
}

/// No host means the default socket; a path, written or percent-encoded, is a socket too.
fn is_local_host(host: &str) -> bool {
    host.is_empty()
        || host.starts_with('/')
        || host.to_ascii_lowercase().starts_with("%2f")
        || matches!(host, "localhost" | "127.0.0.1" | "::1")
}

#[cfg(test)]
mod tests {
    use super::is_local_database;

    #[test]
    fn a_database_on_this_machine_is_local() {
        for url in [
            "postgres://sofie:sofie@127.0.0.1:25432/sofie",
            "postgres://sofie:p%40ss@localhost/shop",
            "postgres://localhost/shop?hostaddr=127.0.0.1",
            "postgresql://[::1]:5432/shop",
            "postgres:///shop",
            "postgres://sofie@/shop?host=/var/run/postgresql",
            "postgres://sofie@%2Fvar%2Frun%2Fpostgresql/shop",
            "sqlite://app.db",
            "sqlite::memory:",
        ] {
            assert!(is_local_database(url), "{url}");
        }
    }

    #[test]
    fn any_other_host_is_not() {
        for url in [
            "postgres://sofie:sofie@db.example.com/shop",
            "postgres://sofie:sofie@10.0.0.5:5432/shop",
            "postgres://localhost,db.example.com/shop",
            "postgres://localhost/shop?host=db.example.com",
            "postgres://sofie:sofie@localhost.example.com/shop",
            "host=localhost dbname=shop",
            "sqlite:app.db",
            "postgres://localhost/shop?hostaddr=10.0.0.5",
            "postgres://localhost/shop?hostaddr=127.0.0.1,10.0.0.5",
            "postgres://localhost/shop?application_name=@db.example.com",
            "postgres://sofie:p@ss@localhost/shop",
            "postgres://localhost/shop?service=production",
            "postgres://localhost/shop?host=/tmp,db.example.com",
            "postgres://localhost/shop?host=%2Ftmp%2Cdb.example.com",
            "postgres://localhost/shop?h%6Fstaddr=10.0.0.5",
            "postgres://localhost/shop?%68ost=db.example.com",
        ] {
            assert!(!is_local_database(url), "{url}");
        }
    }
}

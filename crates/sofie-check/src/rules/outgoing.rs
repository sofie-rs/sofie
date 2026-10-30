use std::path::{Component, Path};

use sofie_model::Module;

use super::{Finding, Rule};

const CLIENT_CRATES: [&str; 14] = [
    "reqwest",
    "ureq",
    "isahc",
    "surf",
    "attohttpc",
    "curl",
    "hyper",
    "hyper_util",
    "h2",
    "rustls",
    "tokio_rustls",
    "native_tls",
    "openssl",
    "sofie_client",
];
/// The HTTP crates a vendor SDK reaches the network through; TLS crates are left out,
/// since libraries also use them only to read certificates.
const PULLED_IN_CLIENTS: [&str; 7] = [
    "reqwest",
    "ureq",
    "isahc",
    "surf",
    "attohttpc",
    "curl",
    "hyper",
];
const SOCKET_MODULE: &str = "net";
const SOCKET_TYPES: [&str; 7] = [
    "TcpStream",
    "TcpSocket",
    "TcpListener",
    "UdpSocket",
    "UnixStream",
    "UnixListener",
    "UnixDatagram",
];
const GATEWAYS: &str = "gateways";

pub(super) fn is_client_crate(package: &str) -> bool {
    CLIENT_CRATES.contains(&package.replace('-', "_").as_str())
}

pub(super) fn is_pulled_in_client(package: &str) -> bool {
    PULLED_IN_CLIENTS.contains(&package.replace('-', "_").as_str())
}

pub(super) fn opens_connection(segments: &[String]) -> bool {
    segments.first().is_some_and(|root| is_client_crate(root)) || names_socket(segments)
}

fn names_socket(segments: &[String]) -> bool {
    segments.windows(2).any(|pair| {
        pair[0] == SOCKET_MODULE && (pair[1] == "*" || SOCKET_TYPES.contains(&pair[1].as_str()))
    })
}

pub(super) fn holds_outgoing_outside_gateways(file: &Path, segments: &[String]) -> bool {
    let names_outgoing = segments.last().is_some_and(|last| last == "Outgoing");
    names_outgoing && !in_gateways(file)
}

fn in_gateways(file: &Path) -> bool {
    let components: Vec<Component<'_>> = file.components().collect();
    components.windows(2).any(|pair| {
        pair[0].as_os_str() == "src"
            && Path::new(pair[1].as_os_str()).file_stem() == Some(GATEWAYS.as_ref())
    })
}

pub(super) fn finding(file: &Path, line: usize, written: &str) -> Finding {
    Finding {
        rule: Rule::Outgoing,
        file: file.to_path_buf(),
        line,
        message: format!(
            "`{written}` opens a socket of its own, which escapes the tracing, egress \
             allowlist and limits that the app's network traffic gets from a gateway's `Outgoing`"
        ),
        fix: "call the vendor through a gateway adapter that holds an `Outgoing`".to_owned(),
    }
}

pub(super) fn pulled_in_finding(
    manifest: &Path,
    line: usize,
    dependency: &str,
    client: &str,
) -> Finding {
    Finding {
        rule: Rule::Outgoing,
        file: manifest.to_path_buf(),
        line,
        message: format!(
            "`{dependency}` pulls in `{client}`, so it opens connections of its own, which escape \
             the tracing, egress allowlist and limits that every request leaving the app gets \
             from a gateway's `Outgoing`"
        ),
        fix: "call the vendor's HTTP API through a gateway adapter that holds an `Outgoing` \
              instead of its SDK"
            .to_owned(),
    }
}

pub(super) fn outside_gateways_finding(file: &Path, line: usize) -> Finding {
    Finding {
        rule: Rule::Outgoing,
        file: file.to_path_buf(),
        line,
        message: "an `Outgoing` is named outside `src/gateways/`, where only adapters, which \
                  the wiring hands one, may hold it"
            .to_owned(),
        fix: "move the code calling the vendor into an adapter under `src/gateways/<vendor>/`"
            .to_owned(),
    }
}

pub(super) fn check_gateways(modules: &[Module]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for gateway in modules.iter().flat_map(|module| &module.gateways) {
        let name = gateway.path.join("::");
        if gateway.adapters.is_empty() {
            findings.push(Finding {
                rule: Rule::Outgoing,
                file: gateway.file.clone(),
                line: gateway.line,
                message: format!("gateway `{name}` has no adapter for the wiring to choose"),
                fix: format!(
                    "implement `{name}` for a vendor's type under `src/gateways/<vendor>/`"
                ),
            });
        }
        for adapter in gateway
            .adapters
            .iter()
            .filter(|adapter| !adapter.is_adapter)
        {
            let ty = adapter.ty.join("::");
            findings.push(Finding {
                rule: Rule::Outgoing,
                file: adapter.file.clone(),
                line: adapter.line,
                message: format!(
                    "`{ty}` implements gateway `{name}` but not `sofie::client::Adapter`, which names \
                     its vendor and is how the wiring builds it"
                ),
                fix: format!("write `impl sofie::client::Adapter for {ty}` beside it"),
            });
        }
    }
    findings
}

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use sofie_core::{Error, Result};

use crate::connect::Connector;
use crate::{Outgoing, Vendor};

/// Trusts only `root`, so a test server's certificate is trusted in that test and nowhere else.
pub fn outgoing(vendor: &'static str, settings: Vendor, root: &[u8]) -> Result<Outgoing> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(root.to_vec()))
        .map_err(|error| Error::internal("trusting the test's root certificate", error))?;
    Outgoing::build(vendor, settings, Connector::trusting(roots))
}

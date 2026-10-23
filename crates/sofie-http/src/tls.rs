use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use sofie_core::{Error, Result, unpoisoned};
use tokio_rustls::TlsAcceptor;

use crate::Serving;

/// A renewed certificate is served within this long of its files changing.
const RECHECK_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Tls {
    pub(crate) acceptor: TlsAcceptor,
}

impl Tls {
    /// `certificate` holds the chain, leaf first; `key` a PKCS#8, PKCS#1 or SEC1 key.
    pub fn from_pem_files(certificate: &Path, key: &Path) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let reloading = Reloading::load(certificate, key, Arc::clone(&provider))?;
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| Error::internal("choosing the TLS protocol versions", error))?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(reloading));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(config)),
        })
    }
}

impl Serving {
    /// Serves HTTPS on the app's address instead of plain HTTP.
    #[must_use]
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }
}

#[derive(Debug)]
struct PemFiles {
    certificate: PathBuf,
    key: PathBuf,
    provider: Arc<CryptoProvider>,
}

type Modified = (SystemTime, SystemTime);

impl PemFiles {
    fn modified(&self) -> Result<Modified> {
        Ok((modified(&self.certificate)?, modified(&self.key)?))
    }

    fn certified(&self) -> Result<CertifiedKey> {
        let chain = CertificateDer::pem_file_iter(&self.certificate)
            .and_then(Iterator::collect::<std::result::Result<Vec<_>, _>>)
            .map_err(|error| {
                Error::internal(
                    format!("reading certificates from {}", self.certificate.display()),
                    error,
                )
            })?;
        if chain.is_empty() {
            return Err(Error::internal_message(format!(
                "{} holds no certificate",
                self.certificate.display()
            )));
        }
        let key = PrivateKeyDer::from_pem_file(&self.key).map_err(|error| {
            Error::internal(
                format!("reading the private key from {}", self.key.display()),
                error,
            )
        })?;
        CertifiedKey::from_der(chain, key, &self.provider).map_err(|error| {
            Error::internal(
                format!("pairing {} with its key", self.certificate.display()),
                error,
            )
        })
    }
}

fn modified(path: &Path) -> Result<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| Error::internal(format!("reading when {} changed", path.display()), error))
}

#[derive(Debug)]
struct Watch {
    modified: Modified,
    next_check: Instant,
}

/// Checks the files' modification times on a handshake at most every `RECHECK_EVERY`,
/// so no task or runtime is needed and an idle server reads nothing.
#[derive(Debug)]
struct Reloading {
    files: PemFiles,
    current: RwLock<Arc<CertifiedKey>>,
    watch: Mutex<Watch>,
}

impl Reloading {
    fn load(certificate: &Path, key: &Path, provider: Arc<CryptoProvider>) -> Result<Self> {
        let files = PemFiles {
            certificate: certificate.to_owned(),
            key: key.to_owned(),
            provider,
        };
        // Read before the contents, so a write in between is seen by the next check.
        let modified = files.modified()?;
        let certified = files.certified()?;
        Ok(Self {
            files,
            current: RwLock::new(Arc::new(certified)),
            watch: Mutex::new(Watch {
                modified,
                next_check: Instant::now() + RECHECK_EVERY,
            }),
        })
    }

    fn current(&self) -> Arc<CertifiedKey> {
        self.reload_if_changed();
        Arc::clone(&unpoisoned::read(&self.current))
    }

    /// A handshake arriving while another checks is served the certificate held.
    fn reload_if_changed(&self) {
        let mut watch = match self.watch.try_lock() {
            Ok(watch) => watch,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        let now = Instant::now();
        if now < watch.next_check {
            return;
        }
        watch.next_check = now + RECHECK_EVERY;
        let reloaded = self.files.modified().and_then(|modified| {
            if modified == watch.modified {
                return Ok(None);
            }
            // Recorded even if the pair fails, so a broken pair is logged once, not every check.
            watch.modified = modified;
            self.files.certified().map(Some)
        });
        match reloaded {
            Ok(None) => {}
            Ok(Some(certified)) => {
                *unpoisoned::write(&self.current) = Arc::new(certified);
                tracing::info!(tls.certificate = %self.files.certificate.display(), "reloaded the TLS certificate");
            }
            Err(error) => {
                tracing::error!(error = %error, tls.certificate = %self.files.certificate.display(), tls.key = %self.files.key.display(), "reloading the TLS certificate failed; serving the one held");
            }
        }
    }
}

impl ResolvesServerCert for Reloading {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    struct Copied {
        directory: PathBuf,
        reloading: Reloading,
    }

    impl Copied {
        fn of(test: &str) -> Self {
            let directory =
                std::env::temp_dir().join(format!("sofie-http-tls-{test}-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            for name in ["localhost.crt", "localhost.key"] {
                std::fs::copy(Path::new(FIXTURES).join(name), directory.join(name)).unwrap();
            }
            let reloading = Reloading::load(
                &directory.join("localhost.crt"),
                &directory.join("localhost.key"),
                Arc::new(rustls::crypto::ring::default_provider()),
            )
            .unwrap();
            Self {
                directory,
                reloading,
            }
        }

        fn replace_key(&self, fixture: &str) {
            let key = self.directory.join("localhost.key");
            std::fs::copy(Path::new(FIXTURES).join(fixture), &key).unwrap();
            let later = self.reloading.watch.lock().unwrap().modified.1 + Duration::from_secs(1);
            File::options()
                .write(true)
                .open(&key)
                .unwrap()
                .set_modified(later)
                .unwrap();
            self.reloading.watch.lock().unwrap().next_check = Instant::now();
        }
    }

    impl Drop for Copied {
        fn drop(&mut self) {
            let removed = std::fs::remove_dir_all(&self.directory);
            // A second panic while a failed test unwinds would abort the run.
            if !std::thread::panicking() {
                removed.unwrap();
            }
        }
    }

    #[test]
    fn changed_files_are_served_from_the_next_check() {
        let copied = Copied::of("changed");
        let before = copied.reloading.current();

        copied.replace_key("localhost.key");

        assert!(!Arc::ptr_eq(&before, &copied.reloading.current()));
    }

    #[test]
    fn unchanged_files_are_not_read_again() {
        let copied = Copied::of("unchanged");
        let before = copied.reloading.current();

        copied.reloading.watch.lock().unwrap().next_check = Instant::now();

        assert!(Arc::ptr_eq(&before, &copied.reloading.current()));
    }

    #[test]
    fn a_failed_reload_keeps_serving_the_certificate_held() {
        let copied = Copied::of("failed");
        let before = copied.reloading.current();

        copied.replace_key("other.key");

        assert!(Arc::ptr_eq(&before, &copied.reloading.current()));
    }
}

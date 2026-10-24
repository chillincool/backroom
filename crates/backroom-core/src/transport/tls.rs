use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use super::TransportError;

/// TLS settings shared by the WebSocket and TCP transports.
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// CA certificates to trust in addition to the Mozilla root store, e.g.
    /// for a self-hosted soju with a private CA, or the e2e test CA.
    pub extra_roots: Vec<CertificateDer<'static>>,
}

impl TlsOptions {
    /// Loads extra CA certificates from a PEM file.
    pub fn with_ca_file(mut self, path: &std::path::Path) -> Result<Self, TransportError> {
        let certs = CertificateDer::pem_file_iter(path)
            .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
            .map_err(|e| TransportError::Tls(format!("{}: {e}", path.display())))?;
        if certs.is_empty() {
            return Err(TransportError::Tls(format!(
                "{}: no certificates found",
                path.display()
            )));
        }
        self.extra_roots.extend(certs);
        Ok(self)
    }

    /// rustls client config with the ring provider, Mozilla's root store and
    /// any extra roots.
    ///
    /// The provider is chosen explicitly so behaviour doesn't depend on which
    /// rustls features other crates happen to enable.
    pub(crate) fn client_config(&self) -> Result<Arc<ClientConfig>, TransportError> {
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        for cert in &self.extra_roots {
            roots
                .add(cert.clone())
                .map_err(|e| TransportError::Tls(format!("bad CA certificate: {e}")))?;
        }
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| TransportError::Tls(e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Arc::new(config))
    }
}

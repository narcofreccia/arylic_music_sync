//! The rustls client configuration for Luci.
//!
//! Three deliberate departures from a normal HTTPS client, all matching what the
//! LSCommunicator SDK does:
//!
//! * **TLS 1.2 only** — the device offers `ECDHE-RSA-AES256-GCM-SHA384` and does
//!   not speak 1.3.
//! * **No server verification** — the device presents a self-signed cert the SDK
//!   never validates ([`AcceptAnyServerCert`]).
//! * **Client authentication** — Luci is mutually authenticated; we present the
//!   embedded LibreWireless cert (`with_client_auth_cert`).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::ResolvesClientCert;
use rustls::crypto::CryptoProvider;
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::TlsConnector;

use crate::error::{AppError, AppResult};
use crate::luci::cert;

/// A server-certificate verifier that accepts anything. The device's cert is
/// self-signed and carries no meaningful name, so there is nothing to check; the
/// mutual-auth client cert is what actually gates the connection. Signature
/// verification is delegated to the crypto provider so the handshake maths still
/// has to hold — we skip identity, not integrity.
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A client-cert resolver that always presents the embedded LibreWireless
/// identity. We resolve directly to a pre-built [`CertifiedKey`] rather than
/// going through `with_client_auth_cert`, because the SDK cert is X.509 **v1**
/// and rustls's client-auth path rejects it as an unsupported version. Like the
/// SDK, we just present the raw cert; the device does not validate it either.
#[derive(Debug)]
struct FixedClientCert(Arc<CertifiedKey>);

impl ResolvesClientCert for FixedClientCert {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// Load the embedded client cert chain and PKCS#8 key.
fn client_identity() -> AppResult<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let mut cert_reader = std::io::BufReader::new(cert::CLIENT_CERT_PEM);
    let certs = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Internal(format!("embedded client cert is unreadable: {e}")))?;
    if certs.is_empty() {
        return Err(AppError::Internal("embedded client cert contained no certificate".into()));
    }

    let mut key_reader = std::io::BufReader::new(cert::CLIENT_KEY_PEM);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| AppError::Internal(format!("embedded client key is unreadable: {e}")))?
        .ok_or_else(|| AppError::Internal("embedded client key contained no private key".into()))?;

    Ok((certs, key))
}

/// Build a Luci `TlsConnector`: TLS 1.2, no server verification, embedded client
/// auth. Cheap to clone; a connector is built once per device in the poller.
pub fn connector() -> AppResult<TlsConnector> {
    // Install a process-wide default provider so anything that reaches for it
    // (or a future rustls call that assumes one) finds ring. Idempotent —
    // ignore the error when another caller won the race.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (certs, key) = client_identity()?;
    let signing_key = provider
        .key_provider
        .load_private_key(key)
        .map_err(|e| AppError::Internal(format!("rustls could not load the client key: {e}")))?;
    let certified = Arc::new(CertifiedKey::new(certs, signing_key));

    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS12])
        .map_err(|e| AppError::Internal(format!("rustls could not enable TLS 1.2: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert(provider)))
        .with_client_cert_resolver(Arc::new(FixedClientCert(certified)));

    Ok(TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_identity_parses() {
        let (certs, _key) = client_identity().expect("embedded cert+key must parse");
        assert!(!certs.is_empty(), "at least one certificate");
    }

    #[test]
    fn connector_builds() {
        connector().expect("the Luci TLS connector must build");
    }
}

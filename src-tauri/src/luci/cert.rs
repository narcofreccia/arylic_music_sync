//! The embedded LibreWireless client certificate.
//!
//! Luci is mutually authenticated: the device asks the client for a certificate
//! during the TLS handshake. This is the **public** cert+key shipped in the
//! open-source `LibreWireless/LSCommunicator` SDK (`Sources/Resources/cert.p12`,
//! subject "APP Certificate", O=Libre Wireless) — every LP10 accepts it, it is
//! not a per-user secret. Extracted to PKCS#8 PEM in `assets/`.
//!
//! Baked into the binary with `include_bytes!` so there is no file to ship or
//! path to resolve at runtime.

/// PEM-encoded client certificate chain (a single RSA-2048 cert).
pub const CLIENT_CERT_PEM: &[u8] = include_bytes!("assets/librewireless_client_cert.pem");

/// PEM-encoded PKCS#8 private key for [`CLIENT_CERT_PEM`].
pub const CLIENT_KEY_PEM: &[u8] = include_bytes!("assets/librewireless_client_key.pem");

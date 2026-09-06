use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

use crate::id::data_dir;
use crate::proto::ALPN;

pub struct Tls {
    pub acceptor: TlsAcceptor,
    pub fingerprint: String,
}

pub fn load_or_create() -> Result<Tls> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let dir = data_dir().join("tls");
    crate::id::ensure_dir(&dir)?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    if !cert_path.exists() || !key_path.exists() {
        let host = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "ddrdesk".into());
        let names = vec![
            host,
            "ddrdesk".into(),
            "localhost".into(),
            "DDRDesk".into(),
        ];
        let certified = rcgen::generate_simple_self_signed(names)?;
        std::fs::write(&cert_path, certified.cert.pem())?;
        std::fs::write(&key_path, certified.key_pair.serialize_pem())?;
        tracing::info!("generated TLS certificate at {}", cert_path.display());
    }

    let cert_pem = std::fs::read(&cert_path)?;
    let key_pem = std::fs::read(&key_path)?;
    let mut cert_reader = std::io::Cursor::new(cert_pem);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse cert")?;
    let mut key_reader = std::io::Cursor::new(key_pem);
    let mut keys: Vec<PrivateKeyDer<'static>> = rustls_pemfile::pkcs8_private_keys(&mut key_reader)
        .map(|k| k.map(PrivatePkcs8KeyDer::from).map(PrivateKeyDer::from))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse key")?;
    if keys.is_empty() {
        anyhow::bail!("no PKCS8 key in {}", key_path.display());
    }

    let fp = fingerprint(&certs[0]);
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, keys.remove(0))?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Tls {
        acceptor: TlsAcceptor::from(Arc::new(cfg)),
        fingerprint: fp,
    })
}

pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let mut h = Sha256::new();
    h.update(cert.as_ref());
    format!("sha256:{}", hex::encode(h.finalize()))
}

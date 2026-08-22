use std::fs::File;
use std::io::{self, BufReader};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme, StreamOwned};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};

use crate::error::MemcacheError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerifyMode {
    None,
    Peer,
}

pub(crate) struct TlsConfig {
    pub(crate) ca_path: Option<String>,
    pub(crate) key_path: Option<String>,
    pub(crate) cert_path: Option<String>,
    pub(crate) verify_mode: VerifyMode,
}

/// A verifier that accepts any server certificate, used for `verify_mode=none`.
#[derive(Debug)]
struct NoVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NoVerifier {
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn pem_error(path: &str, err: rustls_pki_types::pem::Error) -> MemcacheError {
    MemcacheError::IOError(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("failed to parse PEM file {}: {}", path, err),
    ))
}

fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, MemcacheError> {
    let mut reader = BufReader::new(File::open(path)?);
    CertificateDer::pem_reader_iter(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| pem_error(path, err))
}

fn load_key(path: &str) -> Result<PrivateKeyDer<'static>, MemcacheError> {
    let mut reader = BufReader::new(File::open(path)?);
    PrivateKeyDer::from_pem_reader(&mut reader).map_err(|err| pem_error(path, err))
}

fn build_client_config(config: &TlsConfig) -> Result<ClientConfig, MemcacheError> {
    let provider = Arc::new(ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(MemcacheError::TlsError)?;

    let builder = match config.verify_mode {
        VerifyMode::None => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier { provider })),
        VerifyMode::Peer => {
            let mut roots = RootCertStore::empty();
            match &config.ca_path {
                Some(ca_path) => {
                    for cert in load_certs(ca_path)? {
                        roots.add(cert)?;
                    }
                }
                None => {
                    let native = rustls_native_certs::load_native_certs();
                    if native.certs.is_empty()
                        && let Some(err) = native.errors.into_iter().next()
                    {
                        return Err(MemcacheError::IOError(io::Error::other(format!(
                            "failed to load native root certificates: {}",
                            err
                        ))));
                    }
                    for cert in native.certs {
                        roots.add(cert)?;
                    }
                }
            }
            builder.with_root_certificates(roots)
        }
    };

    match (&config.cert_path, &config.key_path) {
        (Some(cert_path), Some(key_path)) => {
            let certs = load_certs(cert_path)?;
            let key = load_key(key_path)?;
            Ok(builder.with_client_auth_cert(certs, key)?)
        }
        _ => Ok(builder.with_no_client_auth()),
    }
}

pub(crate) fn connect(
    host: &str,
    tcp_stream: TcpStream,
    config: &TlsConfig,
) -> Result<StreamOwned<ClientConnection, TcpStream>, MemcacheError> {
    let client_config = Arc::new(build_client_config(config)?);
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| MemcacheError::BadURL(format!("invalid TLS server name: {}", host)))?;
    let conn = ClientConnection::new(client_config, server_name)?;
    let mut stream = StreamOwned::new(conn, tcp_stream);
    // Drive the handshake eagerly so handshake failures surface at connect time
    // rather than on the first read or write.
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock)?;
    }
    Ok(stream)
}

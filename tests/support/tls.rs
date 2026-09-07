//! HTTPS for the mock backend: a throwaway CA and a listener that completes
//! the TLS handshake before handing the connection to axum.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::serve::Listener;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::{ServerConfig, crypto};
use tokio_rustls::server::TlsStream;

/// A CA nobody else trusts, plus a server certificate for `127.0.0.1` it
/// signed.
pub struct TestCa {
    /// The CA certificate as an operator would receive it.
    pub pem: String,
    server: Arc<ServerConfig>,
}

impl TestCa {
    pub fn generate() -> Self {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "anthroxy test CA");
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let server_key = KeyPair::generate().unwrap();
        let server_cert = CertificateParams::new(vec!["127.0.0.1".to_owned()])
            .unwrap()
            .signed_by(&server_key, &issuer)
            .unwrap();
        let server =
            ServerConfig::builder_with_provider(Arc::new(crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![server_cert.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
                )
                .unwrap();
        Self {
            pem: ca_cert.pem(),
            server: Arc::new(server),
        }
    }

    pub fn listener(&self, tcp: TcpListener) -> TlsListener {
        TlsListener {
            tcp,
            acceptor: TlsAcceptor::from(self.server.clone()),
        }
    }
}

pub struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    /// A client that distrusts the certificate aborts the handshake; that
    /// connection is dropped and the next one is awaited.
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((stream, addr)) = self.tcp.accept().await else {
                continue;
            };
            if let Ok(tls) = self.acceptor.accept(stream).await {
                return (tls, addr);
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

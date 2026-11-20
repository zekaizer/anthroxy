//! The HTTP clients backends are reached through, one for each transport.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{BackendConfig, UpstreamConfig, origin};

/// What a request is sent with that is a setting of the HTTP client, not of
/// the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Transport {
    /// HTTP/1.1 even when the server offers HTTP/2.
    pub http1_only: bool,
    /// HTTP/1.1 header names written `Title-Case`.
    pub title_case: bool,
}

impl Transport {
    pub(super) const ALL: [Transport; 4] = [
        Transport {
            http1_only: false,
            title_case: false,
        },
        Transport {
            http1_only: false,
            title_case: true,
        },
        Transport {
            http1_only: true,
            title_case: false,
        },
        Transport {
            http1_only: true,
            title_case: true,
        },
    ];

    fn index(self) -> usize {
        usize::from(self.http1_only) * 2 + usize::from(self.title_case)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("cannot read upstream.ca_certificate {}: {source}", path.display())]
    ReadCaCertificate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("upstream.ca_certificate {}: {source}", path.display())]
    ParseCaCertificate {
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },
    #[error("upstream.ca_certificate {} holds no certificate", path.display())]
    NoCaCertificate { path: PathBuf },
    #[error("cannot build HTTP client: {0}")]
    Http(#[from] reqwest::Error),
}

/// One client for each transport it was built with. Connections are pooled
/// per client.
#[derive(Debug)]
pub struct HttpClients([Option<reqwest::Client>; 4]);

impl HttpClients {
    /// A client for each of `transports`. How every backend is reached: the
    /// configured timeouts, the extra trust anchors from `ca_certificate`,
    /// no redirects, since a redirect would resend the body and the
    /// credential elsewhere, and no proxy but the one a backend names
    /// (ADR-0012), since `http_proxy` in the environment would do the same
    /// to every backend at once. `total` limits a whole request, body
    /// included. `ca_certificate` is read once, whatever `transports` holds.
    /// Assumes `backends` passed validation.
    pub fn build(
        config: &UpstreamConfig,
        backends: &BTreeMap<String, BackendConfig>,
        total: Option<Duration>,
        transports: impl IntoIterator<Item = Transport>,
    ) -> Result<Self, ClientBuildError> {
        let proxies = proxies(backends);
        let certificates = match &config.ca_certificate {
            Some(path) => ca_certificates(path)?,
            None => Vec::new(),
        };
        let mut clients: [Option<reqwest::Client>; 4] = Default::default();
        for transport in transports {
            let slot = &mut clients[transport.index()];
            if slot.is_some() {
                continue;
            }
            let mut builder = reqwest::Client::builder()
                .connect_timeout(config.connect_timeout)
                .tcp_keepalive(Duration::from_secs(60))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none());
            if !proxies.is_empty() {
                let proxies = proxies.clone();
                builder = builder.proxy(reqwest::Proxy::custom(move |url| {
                    origin(url).and_then(|origin| proxies.get(&origin).cloned())
                }));
            }
            for certificate in &certificates {
                builder = builder.add_root_certificate(certificate.clone());
            }
            if transport.http1_only {
                builder = builder.http1_only();
            }
            if transport.title_case {
                builder = builder.http1_title_case_headers();
            }
            if let Some(total) = total {
                builder = builder.timeout(total);
            }
            *slot = Some(builder.build()?);
        }
        Ok(Self(clients))
    }

    /// Panics when no client was built for `transport`.
    pub fn get(&self, transport: Transport) -> &reqwest::Client {
        self.0[transport.index()]
            .as_ref()
            .expect("a client is built for every transport a request travels")
    }
}

/// `client` for every transport, whatever its own settings are.
impl From<reqwest::Client> for HttpClients {
    fn from(client: reqwest::Client) -> Self {
        Self(std::array::from_fn(|_| Some(client.clone())))
    }
}

/// Proxy by backend origin. A backend URL reqwest cannot parse is never
/// requested, so it needs no entry.
fn proxies(backends: &BTreeMap<String, BackendConfig>) -> HashMap<String, reqwest::Url> {
    backends
        .values()
        .filter_map(|backend| {
            let proxy = backend.proxy.as_deref()?;
            let origin = origin(&reqwest::Url::parse(&backend.url).ok()?)?;
            let proxy = reqwest::Url::parse(proxy).expect("validated proxy url");
            Some((origin, proxy))
        })
        .collect()
}

/// Every certificate in the PEM bundle at `path`; an empty bundle is an
/// error because it silently trusts nothing.
fn ca_certificates(path: &Path) -> Result<Vec<reqwest::Certificate>, ClientBuildError> {
    let pem = std::fs::read(path).map_err(|source| ClientBuildError::ReadCaCertificate {
        path: path.to_path_buf(),
        source,
    })?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|source| {
        ClientBuildError::ParseCaCertificate {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if certificates.is_empty() {
        return Err(ClientBuildError::NoCaCertificate {
            path: path.to_path_buf(),
        });
    }
    Ok(certificates)
}

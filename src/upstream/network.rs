//! What backend connections go through, logged whenever a configuration takes
//! effect so a network problem can be read off the service log.

use crate::config::Config;
use crate::config::view::redacted_url;

/// Proxy variables other programs follow and the router ignores (ADR-0012).
const PROXY_VARIABLES: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

#[derive(Debug, PartialEq, Eq)]
pub struct Network {
    pub routes: Vec<Route>,
    /// Proxy variables set in the environment, as `(name, value)`.
    pub ignored: Vec<(String, String)>,
    pub ca_certificate: Option<String>,
    /// Replace the OS certificate store when set (ADR-0008).
    pub ssl_cert_file: Option<String>,
    pub ssl_cert_dir: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Route {
    pub backend: String,
    pub url: String,
    /// `None` connects directly.
    pub proxy: Option<String>,
}

impl Network {
    /// Proxy URLs and variable values come back with their userinfo redacted.
    pub fn read(config: &Config, env: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        Network {
            routes: config
                .backends
                .iter()
                .map(|(name, backend)| Route {
                    backend: name.clone(),
                    url: backend.url.clone(),
                    proxy: backend.proxy.as_deref().map(redacted_url),
                })
                .collect(),
            ignored: PROXY_VARIABLES
                .into_iter()
                .filter_map(|name| Some((name.to_owned(), redacted_url(&set(name)?))))
                .collect(),
            ca_certificate: config
                .upstream
                .ca_certificate
                .as_ref()
                .map(|file| file.display().to_string()),
            ssl_cert_file: set("SSL_CERT_FILE"),
            ssl_cert_dir: set("SSL_CERT_DIR"),
        }
    }
}

/// Logs `config`'s network settings as this process sees them.
pub fn log(config: &Config) {
    let network = Network::read(config, |name| std::env::var(name).ok());
    for route in &network.routes {
        match &route.proxy {
            Some(proxy) => tracing::info!(
                backend = %route.backend,
                url = %route.url,
                %proxy,
                "backend reached through a proxy"
            ),
            None => tracing::info!(
                backend = %route.backend,
                url = %route.url,
                "backend reached directly"
            ),
        }
    }
    for (variable, value) in &network.ignored {
        tracing::info!(
            %variable,
            %value,
            "proxy variable ignored; a backend reached through a proxy names it in `proxy`"
        );
    }
    let upstream = &config.upstream;
    tracing::info!(
        connect_timeout = %humantime::format_duration(upstream.connect_timeout),
        non_stream_timeout = %humantime::format_duration(upstream.non_stream_timeout),
        stream_first_byte_timeout = %humantime::format_duration(upstream.stream_first_byte_timeout),
        stream_idle_timeout = %humantime::format_duration(upstream.stream_idle_timeout),
        retries = upstream.retries,
        ca_certificate = network.ca_certificate.as_deref(),
        ssl_cert_file = network.ssl_cert_file.as_deref(),
        ssl_cert_dir = network.ssl_cert_dir.as_deref(),
        "upstream connections"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(backend: &str, url: &str, proxy: Option<&str>) -> Route {
        Route {
            backend: backend.to_owned(),
            url: url.to_owned(),
            proxy: proxy.map(str::to_owned),
        }
    }

    #[test]
    fn reads_each_backend_route_and_the_proxy_variables_it_ignores() {
        let text = r#"
[server]
token = "t"

[upstream]
ca_certificate = "/etc/anthroxy/corp.pem"

[backends.gw]
url = "https://gw.corp"
proxy = "http://alice:proxy-secret@proxy.corp:3128"

[backends.vllm]
url = "http://10.0.0.5:8000"

[[models]]
id = "m"
backend = "gw"
"#;
        let config = Config::parse(text, |_| None).unwrap();
        let env = |name: &str| match name {
            "HTTPS_PROXY" => Some("http://bob:env-secret@proxy.corp:3128".to_owned()),
            "no_proxy" => Some("localhost,.corp".to_owned()),
            "HTTP_PROXY" => Some(String::new()),
            "SSL_CERT_FILE" => Some("/etc/ssl/corp.pem".to_owned()),
            _ => None,
        };
        let network = Network::read(&config, env);
        assert_eq!(
            network.routes,
            [
                route(
                    "gw",
                    "https://gw.corp",
                    Some("http://<redacted>@proxy.corp:3128")
                ),
                route("vllm", "http://10.0.0.5:8000", None),
            ]
        );
        assert_eq!(
            network.ignored,
            [
                (
                    "HTTPS_PROXY".to_owned(),
                    "http://<redacted>@proxy.corp:3128".to_owned()
                ),
                ("no_proxy".to_owned(), "localhost,.corp".to_owned()),
            ],
            "an empty variable is not set"
        );
        assert_eq!(
            network.ca_certificate.as_deref(),
            Some("/etc/anthroxy/corp.pem")
        );
        assert_eq!(network.ssl_cert_file.as_deref(), Some("/etc/ssl/corp.pem"));
        assert_eq!(network.ssl_cert_dir, None);
    }
}

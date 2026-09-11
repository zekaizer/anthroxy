# 0012. Per-backend egress proxy

## Status

accepted

## Context

The egress client ignores `http_proxy` / `https_proxy` / `all_proxy`. An ambient variable, routine on a corporate WSL2 host, sent every backend request, loopback included and the credential with it, through a proxy the configuration never named. That stays.

It left no way to reach a backend that answers only through a proxy. On a corporate host an internal gateway resets a direct connection and answers through the proxy, while a vLLM backend served by the same router is reached directly. Every request to the gateway fails as `unreachable`, and nothing in the file can fix it. One process serves both backends, so a process-wide setting cannot describe the network.

reqwest chooses a proxy per connection destination (scheme, host and port) and pools connections by that origin; the request path takes no part in either.

## Decision

- `backends.<name>.proxy` names the proxy that backend is reached through, as an `http://` or `https://` URL. Userinfo in it is sent as `Proxy-Authorization: Basic`; `${NAME}` expansion keeps a password out of the file. Unset means a direct connection.
- An `https` backend is tunnelled with `CONNECT`, so TLS still ends at the backend and is verified as ADR-0008 says. An `http` backend's requests go to the proxy in absolute form.
- The proxy belongs to the backend's origin. Backends sharing an origin must name the same proxy or none; a mismatch is a validation error, because connections to one origin are pooled together.
- Proxy environment variables stay ignored, `NO_PROXY` included. There is no router-wide proxy: a proxy carries only the backends that name it.
- SOCKS is not built in. A proxy URL with a path, query or fragment is rejected.
- The mapping lives in the one egress client builder, so `serve`, `check` and the console's probe reach a backend the same way.
- Wherever the configuration is shown or logged, the userinfo of a proxy URL is redacted.

## Consequences

- A host that relied on `https_proxy` moves the proxy into the file, on each backend that needs it.
- A TLS-intercepting proxy on the tunnel still needs its CA (ADR-0008).
- Two backends on one origin cannot split between direct and proxied; addressing one of them by another host name or port separates them.
- A proxy that refuses reads as the backend being unreachable; the proxy's error is in the error's source chain.

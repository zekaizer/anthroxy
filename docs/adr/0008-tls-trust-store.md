# 0008. TLS trust store for backend connections

## Status

accepted

Supersedes the egress TLS root choice in ADR-0001; the rest of ADR-0001 stands.

## Context

ADR-0001 built the egress client with reqwest's `rustls-tls` feature, which
trusts only the Mozilla root set compiled into the binary (`webpki-roots`). The
system certificate store and `SSL_CERT_FILE` are never consulted. On a corporate
network the backend's chain ends in a private CA, either because the backend
itself carries a corporate certificate or because a TLS-intercepting proxy
re-signs every connection. Every such connection fails with
`invalid peer certificate: UnknownIssuer`, and nothing in the configuration can
fix it. ADR-0001 noted the gap and named `rustls-tls-native-roots` as the
answer.

Two deployment shapes have to work: a machine whose administrator already
installed the corporate CA into the OS store (`update-ca-certificates` on
Ubuntu, the keychain on macOS), and a machine where the operator cannot or will
not touch the OS store and has the CA only as a file.

## Decision

- The client trusts the union of two root sets: the compiled-in Mozilla roots
  (`rustls-tls-webpki-roots`) and the platform store
  (`rustls-tls-native-roots`). Keeping the bundled set means a host with an
  empty store still reaches public endpoints; adding the platform store means a
  CA installed the way the OS documents is trusted without router configuration.
- The platform loader is `rustls-native-certs`: on Linux it reads the
  OpenSSL-style bundle and directory, on macOS the system keychains, and on
  every platform `SSL_CERT_FILE` / `SSL_CERT_DIR` replace the store when set.
- `upstream.ca_certificate` optionally names a PEM file whose certificates are
  added as further trust anchors. It applies to every backend, because they
  share one client. A file that cannot be read or holds no certificate is a
  client build error, so `anthroxy check` and `serve` fail at startup rather
  than on the first request. `${NAME}` expansion and a leading `~/` apply as for
  other paths.
- There is no switch to disable verification. A backend that cannot be
  authenticated is not proxied; the credential it would receive is the
  operator's.

## Consequences

- A corporate CA can be supplied three ways, in order of preference: install it
  in the OS store; point `SSL_CERT_FILE` at it; set `upstream.ca_certificate`.
  Only the last is visible in the configuration file, so it is the one to use
  when the file is synced between machines and the CA travels with it.
- `SSL_CERT_FILE` exported in a shell is not seen by the systemd user service
  (ADR-0006); it needs `Environment=` in the unit. The OS store and
  `ca_certificate` have no such dependency.
- The binary stays statically linkable (ADR-0007): the native loader reads files
  on Linux; on macOS it links the Security framework, which is a system library.
- The platform loader fails the client build only when the store yields no valid
  certificate while holding unparsable ones; an empty store is not an error. A
  broken store therefore surfaces at startup rather than as `UnknownIssuer`
  later.
- The test suite carries a TLS mock backend with a generated CA (`rcgen`) so
  both trust paths are exercised without touching the host's store.

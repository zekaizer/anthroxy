# Running and debugging the router

Keeping anthroxy running unattended, and finding out what happened when a
request did not go the way it should.

## As a systemd user service (Linux and WSL2)

```sh
anthroxy service install     # writes ~/.config/systemd/user/anthroxy.service, enables it, enables linger
anthroxy service status      # ✓/✗ per check; exit 1 if anything is wrong
anthroxy service reload      # re-read the config (systemctl --user reload)
journalctl --user -u anthroxy -f
anthroxy service uninstall
```

Requirements: systemd enabled in the distribution (`[boot] systemd=true` in
`/etc/wsl.conf`, default on recent Ubuntu). WSL still shuts the VM down when
idle unless `%USERPROFILE%\.wslconfig` sets `[wsl2] vmIdleTimeout=-1`.

From Claude Code on the Windows host, use the distribution's address
(`hostname -I` inside WSL) in `ANTHROPIC_BASE_URL`, or `localhost` when WSL
networking is set to `mirrored`.

## When something breaks

- **Logs.** Text by default, `--log-format json` for shippers. Each request runs
  in a span `request{id=… method=… path=… model=… backend=…}`; the lines you
  will look for are `routed`, `upstream responded` (status, attempts, latency),
  `response body complete` (bytes, chunks, time to first byte) and the `WARN`s:
  retries, credential refreshes, client disconnects, upstream errors.
- **Credentials.** `anthroxy credential` runs every backend's credential command
  through the same code the router uses and reports what came back; `--reveal`
  prints values unmasked instead of `sk-a…9999 (108 chars)`. A command that
  works in your shell but not as a service is an environment difference: on
  Linux, `--as-service` runs it a second time in a transient unit under the
  systemd user manager — the environment `anthroxy.service` starts with — and
  prints the delta (`PATH` in full, other variables by name). Typical causes:
  the interpreter is `dash`, not your login shell; `PATH` has none of the
  directories your profile adds; a keychain or agent socket
  (`DBUS_SESSION_BUS_ADDRESS`, `SSH_AUTH_SOCK`) is absent.
- **Body capture.** Set `logging.body_dir` or pass `--body-dir DIR` to `serve`.
  Each request gets `<DIR>/<time>-<request id>/` with `request.json` (exactly
  what went upstream), `response.json|sse|bin` (exactly what came back) and
  `meta.json` (routing, headers with credentials redacted, the client's message
  count and the turn's prompt cut to one line, where a message the user sent
  while the model was working or a slash or shell command counts as a prompt and
  Claude Code's notices (hook feedback, task notifications, command output,
  compaction summaries) do not, and, when the last message carries no prompt,
  what it sends instead, status, timings, outcome). Entries older than
  `logging.body_retention` (default 7 days) are deleted at startup and every 10
  minutes; set it to `0s` to keep everything. A recording holds the whole
  conversation, so on Unix the router creates the directory and everything under
  it owner-only; a `body_dir` that already exists keeps the mode it has.
- **Console.** The Requests tab has each failure's upstream error body and hints
  without reading the journal; Tools → Probe runs credential commands and model
  lists inside the service process, which is where environment differences show;
  a request's detail opens its body recording.
- **Levels.** `--log-level debug` (or `trace`) applies to the router only. To
  see the HTTP client internals, name them:
  `--log-level "anthroxy=debug,hyper=debug,h2=debug"`.

The console's Requests tab shows most of this without a terminal; see [The web
console](console.md).

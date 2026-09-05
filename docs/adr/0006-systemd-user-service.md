# 0006. Run as a systemd user service on WSL2

## Status

accepted

## Context

The router runs inside WSL2 (Ubuntu) and must be reachable from Claude Code on the Windows host as well as inside the distribution. It should be up whenever the machine is, without the operator opening a shell first. WSL2 distributions can run systemd (`[boot] systemd=true` in `/etc/wsl.conf`, default on recent Ubuntu images); user units need `loginctl enable-linger` to start without a login session. Alternatives considered: a system unit (needs root and hides the operator's `~/.config`), Windows Task Scheduler launching `wsl.exe` (works without systemd but is configured outside the distribution), and no service at all (operator runs `serve` in a terminal).

## Decision

- `claude-router service install` writes `~/.config/systemd/user/claude-router.service`, runs `systemctl --user daemon-reload`, `systemctl --user enable --now`, then `loginctl enable-linger`. `uninstall` reverses it; `status` shows `systemctl --user status`.
- The unit runs `<absolute exe> --config <absolute config> serve` with `Restart=on-failure`, so the router follows the binary and file the operator installed it with.
- The command is Linux-only and reports a clear error elsewhere; `--print` renders the unit on any platform for inspection or manual installation.
- Binding to `0.0.0.0` (ADR-0003 defaults) is what makes the Windows host reach the service under both NAT and mirrored networking; the service itself does not touch networking.

## Consequences

- One command makes the router survive shell exits and WSL restarts, but only if systemd is enabled in the distribution; `install` surfaces the `systemctl` error verbatim when it is not.
- WSL2 still stops the whole VM when idle (`vmIdleTimeout`) unless the operator disables that in `.wslconfig`; the service cannot prevent it and the documentation says so.
- Logs go to the user journal (`journalctl --user -u claude-router`), which is where the tracing output is meant to be read.
- Moving the binary or the configuration file requires `service install` again; the unit embeds absolute paths on purpose.

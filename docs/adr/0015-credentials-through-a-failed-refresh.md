# 0015. Credentials through a failed refresh

## Status

accepted

Supersedes, for `command` credentials that report `expires_at`, ADR-0004's rule
that a cached value is never used past its refresh window even if the new run
fails; the rest of ADR-0004 stands.

## Context

A `command` credential is re-run every `refresh` (default 5 minutes). When a run
fails, every request to that backend fails with a 502 until a run succeeds, even
when the previous value is still valid. With `output = "json"` the command
reports the value's `expires_at`, and a token helper typically hands out a token
that lasts much longer than the refresh interval. A brief failure of the helper
or of the identity service behind it then takes the backend down for no reason.

ADR-0004 refused stale values because a token past its expiry turns into
upstream 401s, which say less than the router's own error. That reasoning holds
when the router cannot tell whether the value still works. It does not hold for
a value whose expiry the command reported and which has not yet reached it.

A failing helper can also be slow to fail: a hanging command costs its full
`timeout` on every attempt.

## Decision

- When a run fails and the cached value reported an `expires_at` more than the
  expiry margin (two minutes) away, requests keep getting the cached value. A
  warning is logged, and the failed run appears in the status history with its
  error.
- While a value is served this way, the command is run again at most every 30
  seconds. Requests in between get the cached value without waiting on a run.
- A backend that rejects the value with 401/403 invalidates it as before
  (ADR-0004), which ends the fallback. The re-acquisition that follows reports
  its own failure.
- A value within the expiry margin, or without a reported `expires_at`
  (`output = "text"`), is not served past its refresh window. ADR-0004's
  behaviour applies unchanged.

## Consequences

- A token helper outage shorter than the remaining lifetime of the current token
  no longer interrupts requests.
- A value revoked before its reported expiry keeps being sent until the backend
  rejects it. The rejection then goes through the 401 refresh and ends in the
  credential error, not a silent 401.
- `output = "text"` gains nothing. A command that can report an expiry should
  use `output = "json"`.

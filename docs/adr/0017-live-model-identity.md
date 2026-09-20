# 0017. Live model identity on any backend

## Status

accepted

## Context

`kind = "passthrough"` (ADR-0016) is the only backend that fills `GET /v1/models` from the origin. Ordinary `anthropic` and `openai` backends expose only `[[models]]`. An OpenAI-compatible origin such as xAI already lists its ids at `GET /v1/models`, but that body is `{id, object, created}`, not Anthropic `{id, type, display_name, created_at}`, so Claude Code never sees them unless the operator copies each id into the file. Passthrough could not read that OpenAI shape either.

## Decision

- `backends.<name>.live_models = true` fetches `GET {url}{models_path}` and publishes those ids as Anthropic model identity (`id`, `type = "model"`, `display_name`, `created_at`). `kind = "passthrough"` implies this.
- The same decoder accepts Anthropic and OpenAI list shapes. Missing `display_name` is the id; OpenAI `created` becomes `created_at`.
- Configured `[[models]]` still win on the same id. A backend with `live_models` may omit `[[models]]`.
- Live fetches on a non-passthrough backend use that backend's credential, not the client's `Authorization`.

## Consequences

- A Grok/`openai` backend can show whatever xAI lists without a row per model.
- Passthrough origins that speak OpenAI `/v1/models` now appear in the picker instead of being dropped as unreadable JSON.
- Two live backends can publish the same id; the first in backend-name order wins after configured ids.

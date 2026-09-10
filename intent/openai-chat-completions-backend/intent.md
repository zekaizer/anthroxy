# Intent: openai-chat-completions-backend
Author: Luke Lee (anthroxy maintainer). Status: draft. Date: 2026-09-10.

## Problem
Our in-house LLM service exposes only the OpenAI API. anthroxy can only attach backends that speak the Anthropic Messages API, so this service cannot be used from Claude Code at all. There is no workaround. Today the service is used only through OpenCode. A single user (the author) hits this in every daily Claude Code session.

## Proposed outcome
Models from the in-house LLM service can be selected and used in Claude Code exactly like the existing Anthropic models. From the user's point of view, whether the backend speaks the OpenAI API or the Anthropic API is indistinguishable.

## Affected users and systems
- The author (a Claude Code user)
- The in-house LLM service (OpenAI Chat Completions API only — confirmed from the OpenCode config, which uses `@ai-sdk/openai-compatible`)
- The anthroxy router
- Existing backends (vLLM, api.anthropic.com) must keep working unchanged

## Constraints
- Rust edition 2024; the only ingress is Claude Code speaking the Anthropic Messages API; runtime is WSL2 (CLAUDE.md)
- The target OpenAI endpoint is Chat Completions (`/v1/chat/completions`)
- Anthropic ↔ OpenAI translation goes through an intermediate representation (IR) layer (author's decision)

## Out of scope
- Responses API (`/v1/responses`) support
- Ingress from clients other than Claude Code (OpenCode, etc.)

## Open questions
- How far Anthropic-specific features (extended thinking, prompt caching, beta headers) are mapped onto the OpenAI side — in scope for this work, but the extent is decided at the requirements stage (author)

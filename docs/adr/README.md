# Architecture Decision Records

One file per decision: `NNNN-short-title.md`, zero-padded and sequential. Nygard template: Title, Status, Context, Decision, Consequences.

Write an ADR only when the change is one of:

1. A library, framework, or toolchain choice.
2. A change to an externally visible API or contract.
3. An architectural decision that is hard to reverse.

Rules:

- Write it before implementation and land it in the same PR as the implementation.
- An `accepted` ADR is immutable. To change it, set its status to `superseded by ADR-N` and write a new one.
- Record the decision and its reasoning, not the work log.

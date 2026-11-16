# Maintainer documentation

This directory holds two kinds of documentation:

- `docs/src/` is the user-facing VitePress site published at
  [biwa.takuk.me](https://biwa.takuk.me/). VitePress builds only `docs/src/`
  (`srcDir: "src"` in `docs/.vitepress/config.ts`), so nothing else in `docs/` is
  published.
- The Markdown files directly in `docs/` and its other subdirectories are for
  maintainers and agents: the plan, decisions, delivery and dated research. They are
  read on GitHub.

Read this page, then the [plan](plan.md), before starting work.

## What belongs where

| Document                           | Owns                                                  | Update rule                                                                  |
| ---------------------------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------- |
| [Plan](plan.md)                    | Roadmap, current status, open questions for the owner | The single work tracker; update in the PR that changes a gate or the roadmap |
| [ADRs](adr/README.md)              | Large design decisions, with context and alternatives | Amend or supersede; never rewrite accepted text                              |
| [Delivery](delivery.md)            | CI, docs deployment, review and merge procedure       | Update with the workflow change                                              |
| [Research](research/README.md)     | Dated observations and primary sources                | Append or supersede; never present old evidence as current                   |
| [Agent instructions](../AGENTS.md) | Commands, code conventions and agent workflow rules   | Update with the tooling or rule change                                       |
| [User docs site](src/index.md)     | Installation, configuration, CLI reference for users  | Derived: update with the code or document that owns each fact                |

Code, `mise.toml`, `tasks.toml` and the workflows under `.github/workflows/` are the
authority for what the project does now. A document that disagrees with them is
stale; fix the document in the same PR.

## Recording decisions

Every design decision the owner makes is recorded in this repository. A large one
gets an [ADR](adr/README.md); a smaller one goes in the `docs/` page that owns the
topic, with the date and time (UTC) of the owner's message, for example
"(owner, 2026-11-11 09:20 UTC)". A decision that exists only in a chat, a thread or
an agent's memory is not recorded. Land the record with the implementation PR, or
as its own docs PR.

While a design is still being discussed, write no ADR, code or PR for it until the
owner approves it. Mark unresolved choices as open questions in the
[plan](plan.md#open-questions) rather than writing them as decisions.

Keep one authoritative location for each fact and link to it instead of copying it.
Do not keep a chat transcript, a chronological decision log or a status page per
PR. Verification of a single change belongs in its PR.

This layout follows [risu729/pateat `docs/`](https://github.com/risu729/pateat/tree/main/docs)
and [Kogane ADR 0041](https://github.com/risu729/kogane/blob/main/docs/adr/0041-documentation-scope.md),
with fewer document classes because biwa is smaller.

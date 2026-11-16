# Plan

Status on 2026-11-16 (main
[4c6bcac](https://github.com/risu729/biwa/commit/4c6bcac)): biwa
[v1.0.0](https://github.com/risu729/biwa/releases/tag/v1.0.0) is released. The CLI
runs commands on UNSW CSE servers over SSH (`biwa run`), synchronizes the project
(`biwa sync`, `biwa pull`), sets up SSH keys (`biwa setup-ssh`), forwards
environment variables, dispatches configured [direct commands](src/direct-commands.md)
such as `1511 autotest lab01` without typing `biwa run`, and cleans stale remote projects (`biwa clean`). The user
docs site is deployed at [biwa.takuk.me](https://biwa.takuk.me/). Releases are cut
by release-plz and binaries published by cargo-dist; dependency updates come from Renovate.

## Roadmap

The owner has not set the roadmap phases for biwa yet. Until the owner does, this
section stays empty and no phase is assumed; see [open questions](#open-questions).

When the owner sets the phases, record them here with the date and time (UTC) of
the owner's message and, for each phase, what has to be true for it to be done.

## Existing issues

Open issues on 2026-11-16, unordered. This list is a snapshot for planning, not a
roadmap or a priority order.

| Issue                                                | Title                                                           |
| ---------------------------------------------------- | --------------------------------------------------------------- |
| [#211](https://github.com/risu729/biwa/issues/211)   | feat: mise integration for remote command environments          |
| [#243](https://github.com/risu729/biwa/issues/243)   | feat(sync): Mutagen engine integration                          |
| [#263](https://github.com/risu729/biwa/issues/263)   | feat: privacy-preserving telemetry collection                   |
| [#292](https://github.com/risu729/biwa/issues/292)   | test: improve CLI and sync coverage using coverage-guided gaps  |
| [#366](https://github.com/risu729/biwa/issues/366)   | perf(ssh): investigate cross-invocation connection multiplexing |
| [#369](https://github.com/risu729/biwa/issues/369)   | feat(ssh): local and remote port forwarding                     |
| [#373](https://github.com/risu729/biwa/issues/373)   | Full interactive TTY support for biwa run and biwa shell        |
| [#653](https://github.com/risu729/biwa/issues/653)   | feat(direct): support prefix and regex default args             |
| [#918](https://github.com/risu729/biwa/issues/918)   | feat: support regex patterns for direct commands                |
| [#1375](https://github.com/risu729/biwa/issues/1375) | ci: publish signed packslip manifests for binary releases       |

## Open questions

Questions for the owner. Each one moves into the owning page or an ADR once the
owner answers it.

1. **Roadmap phases.** What are the phases, and what is phase 1? Which of the
   [existing issues](#existing-issues) belong to it?
2. **Tracker.** Does this page stay the tracker with issues only for later
   phases, as in [risu729/pateat](https://github.com/risu729/pateat/blob/main/docs/plan.md),
   or do the existing GitHub issues stay the tracker?
3. **Checks that need the owner's environment.** CI and cloud sessions test
   against the Docker SSH servers in `docker-compose.yml`. Checks against a real
   UNSW CSE account are proposed to go to a local session on the owner's machine,
   as the agent workflow in [AGENTS.md](../AGENTS.md#owner-conventions) says for
   real accounts. Which checks need that, and may an agent run biwa against the
   owner's CSE account at all?

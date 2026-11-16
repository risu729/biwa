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

## Work tracking

There are no roadmap phases. Work is tracked in
[GitHub issues](https://github.com/risu729/biwa/issues) (owner, 2026-11-16 17:27 UTC:
「phaseはなくていいや。issuesでやります」). This page does not keep a second list of
them.

## Real UNSW CSE account checks

Agents do not test against a real UNSW CSE account for now (owner, 2026-11-16 17:27
UTC: 「unsw cseアカウントでの確認は今やらなくていい」). Tests run against the Docker SSH
servers in `docker-compose.yml`, in CI and in cloud sessions.

## Open questions

Questions for the owner. Each one moves into the owning page or an ADR once the
owner answers it.

None.

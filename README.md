<p align="center">
  <img src="docs/src/public/icon.svg" alt="biwa" width="144" />
</p>

<h1 align="center">biwa</h1>

<p align="center">
  Develop locally and run commands seamlessly on UNSW CSE servers.
</p>

<p align="center">
  <a href="https://biwa.takuk.me/">Documentation</a> ·
  <a href="https://biwa.takuk.me/getting-started">Getting started</a> ·
  <a href="https://github.com/risu729/biwa/releases">Releases</a>
</p>

biwa keeps your preferred editor and tooling on your own machine, synchronizes
your project efficiently, and runs CSE-specific commands remotely. It is a
modern, actively maintained successor to
[`cserun`](https://cserun.bojin.co/).

## Highlights

- Edit locally while running `autotest`, `give`, and other commands on CSE.
- Synchronize only changed files with smart remote path handling.
- Forward environment variables and standard input when needed.
- Clean up stale remote projects to stay within CSE disk quotas.

## Install

Using [mise](https://mise.jdx.dev/) (recommended):

```sh
mise use -g github:risu729/biwa
```

Alternatively, install from crates.io:

```sh
cargo install biwa
```

Windows users should run biwa inside
[WSL2](https://learn.microsoft.com/en-us/windows/wsl/install).

## Quick start

Initialize a project, configure your CSE account in the generated `biwa.toml`,
then run a command:

```sh
biwa init
biwa run 1511 autotest lab01
```

See the [getting started guide](https://biwa.takuk.me/getting-started) for SSH
setup and configuration, or browse the full
[CLI reference](https://biwa.takuk.me/cli/).

## Development

This repository uses mise for tool versions, tasks, and project dependency
setup.

```sh
mise install
mise deps
```

Common workflows:

```sh
mise run build
mise run test
mise run check --lint
```

See the [contributing guide](https://biwa.takuk.me/contributing) for the full
development workflow.

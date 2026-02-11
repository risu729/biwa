# About

**biwa** is a CLI tool for executing commands on UNSW CSE servers from your local machine. It allows you to work locally with your preferred editor and tools while seamlessly running code on CSE infrastructure.

## Evolution from cserun

biwa is inspired by [cserun](https://github.com/Bogay/cserun), a pioneering tool created by community members to simplify remote development on CSE infrastructure. We're grateful to the cserun project and its contributors.

While biwa builds on the foundation laid by cserun, it introduces several improvements:

- **Rust implementation**: Complete rewrite in Rust for blazing-fast performance, type safety, and reliability.
- **Modern tooling**: Deep integration with contemporary development tools like **mise** and modern terminal workflows.
- **Active maintenance**: Maintained by @risu729 with ongoing updates and community support.

## Why biwa?

Working with remote servers often involves juggling multiple tools, manual file synchronization, and complex SSH configurations. biwa simplifies this by providing:

- **Unified interface**: A single command (`biwa run`) to execute remote commands.
- **Transparent operation**: It feels like running commands locally—biwa handles the details.
- **Configuration management**: Easy setup with `biwa.toml` or `biwa.json`.
- **Integration ready**: detailed configuration options for advanced users.

## Core Philosophy

biwa is designed for the common case: you want to **edit code locally** with your preferred tools and setup, but **run it on remote CSE infrastructure** for testing, compilation, or submission. It optimizes for this workflow without forcing you into a specific editor or requiring heavy remote resources.

# AGENTS.md

This file is for agentic coding agents working in this repository.
It captures the commands, conventions, and guardrails that are actually used here.

## Project Overview

- Language: Rust for the CLI in `src/` and tests in `tests/`.
- Tooling: `mise` manages tool versions and task aliases.
- Docs site: VitePress app in `docs/`, using Bun, TypeScript, oxlint, and oxfmt.
- CI entrypoints: see `tasks.toml` and `.github/workflows/ci.yml`.
- Rust edition: 2024.
- Preferred local runner for common workflows: `mise run <task>`.

## Repository-Specific Instruction Files

- `AGENTS.md`: this file.
- `.github/copilot-instructions.md`: not present.
- `.cursorrules`: not present.
- `.cursor/rules/`: not present.
- If any of those files are added later, treat them as higher-priority repo instructions and merge them into your behavior.

## Setup And Environment

- `mise install` and `mise prep` effectively happen as part of normal `mise run ...` usage, so you usually do not need to run them manually.
- Rust is pinned in `mise.toml`; CI force-reinstalls Rust through mise.
- Bun is required for docs tasks.
- Use `mise run <task>` for named tasks from `tasks.toml`.
- Use `mise x -- <command>` when you need mise-managed tools or env vars loaded explicitly for an arbitrary command.
- `mise x --` is usually unnecessary, but if a binary is not found or the environment looks wrong, retry with it.
- `target/debug/` is added to PATH via `mise.toml`, so local `biwa` builds can be invoked directly in a mise environment.
- `pitchfork.toml` enables automatic background daemons when you enter the directory.
- The required SSH test server normally starts automatically via `pitchfork`; if it is not running, use `pitchfork start --all`.
- Do not run `docker compose up` manually for the test server; prefer `pitchfork`.

## High-Value Commands

### Build

- `mise run build` - canonical project build.
- `cargo build` - direct equivalent.
- `cargo build --release` - release build when needed.
- `mise run docs:build` - build the VitePress docs site.

### Test

- `mise run test` - full Rust test suite.
- `cargo test` - direct equivalent.
- `mise run test:coverage` - coverage run with tarpaulin.
- `cargo tarpaulin` - direct equivalent coverage command.

### Run A Single Test

- Single unit test by name: `cargo test cli_run_subcommand`.
- Single integration test function: `cargo test --test ssh_e2e_run e2e_run_command`.
- Single test file / integration target: `cargo test --test ssh_e2e_run`.
- Single module-oriented match: `cargo test nested_path_resolution`.
- Preserve test stdout/stderr when debugging: `cargo test e2e_run_command -- --nocapture`.
- Run one exact test name if substring matching is risky: `cargo test e2e_run_command -- --exact`.

### Snapshots

- `mise run test:update-snapshot` - accept Insta snapshots and remove unreferenced ones.
- Snapshot files live under `src/cli/snapshots/`.
- When changing output intentionally, update snapshots in the same change.

### Lint / Format / Check

- `mise run check` - umbrella task for formatters and linters; may autofix when `LINT` is unset.
- `LINT=true mise run check` - CI-like check mode without formatting/fix writes.
- `mise run check:clippy` - clippy, with autofix first unless `LINT=true`.
- `mise run check:rustfmt` - Rust formatting.
- `mise run check:cargo-deny` - dependency / license / advisory checks.
- `mise run check:actionlint` - GitHub Actions lint.
- `mise run check:ghalint` - GitHub Actions policy lint.
- `mise run check:pinact` - pinned action verification.
- `mise run check:zizmor` - GitHub Actions security lint.
- `mise run check:yamllint` - YAML lint.
- `mise run check:oxfmt` - format Markdown, JSON, YAML, TOML, TS, JS, CSS, Vue.
- `mise run check:typos` - spellcheck.
- `mise run check:oxlint` - docs JS/TS/Vue lint.
- `mise run check:tsc` - docs TypeScript checks.

### Docs And Rendered Artifacts

- `mise run docs:dev` - VitePress dev server, though `pitchfork` usually keeps it running for you.
- `mise run docs:preview` - preview built docs.
- `mise run render` - generate all rendered artifacts.
- `mise run render:schema` - write `schema/config.json`.
- `mise run render:usage` - write `biwa.usage.kdl` and docs CLI reference pages.
- Generated files are intentionally read-only in `.vscode/settings.json`: `Cargo.lock`, `biwa.usage.kdl`, `docs/src/cli/*`, `schema/*`.
- Always prefer regenerating these files instead of hand-editing them.
- The docs dev server is also auto-managed by `pitchfork`; when you need to inspect it, just use `xh localhost:5173`.

## Test Environment Notes

- E2E tests use the local SSH container described in `docker-compose.yml`.
- CI starts an SSH service container with username `testuser`, password `password123`, port `2222`.
- Shared integration helpers are in `tests/common/mod.rs`.
- Tests install `color_eyre` globally at startup for improved diagnostics.
- Tests that mutate environment variables use `#[serial]` and cleanup guards; preserve that pattern.

## Rust Style Guidelines

### Formatting

- Run `cargo fmt` or `mise run check:rustfmt` after Rust edits.
- Rust formatting uses hard tabs; `rustfmt.toml` sets `hard_tabs = true`.
- Match existing layout and let rustfmt own spacing and wrapping.

### Imports

- Keep imports explicit and grouped compactly.
- Prefer crate-local imports like `use crate::Result;` and `use crate::{...};`.
- Alias trait imports with `as _` when imported only for method resolution, e.g. `WrapErr as _`, `Digest as _`, `StreamExt as _`.
- Alias conflicting types when it improves clarity, e.g. `std::io::Error as IoError`.
- Do not introduce wildcard imports.

### Types And APIs

- Use `crate::Result<T>` for crate code; clippy disallows `eyre::Result` and `color_eyre::Result` outside the central alias in `src/main.rs`.
- Integration tests define their own local `Result<T>` alias in `tests/common/mod.rs`; follow that existing pattern in test code.
- Prefer strong domain types and enums over stringly typed flags.
- Derive traits liberally when the type is configuration or value-like: `Debug`, `Clone`, `Serialize`, `Deserialize`, `JsonSchema`, `PartialEq`, `Eq`, `Default` when appropriate.
- Use `Path` / `PathBuf` for filesystem paths.

### Error Handling

- Prefer propagating errors with `?`.
- Add context with `WrapErr` / `wrap_err_with` for IO, parsing, network, and subprocess boundaries.
- Use `bail!` for user-facing early exits.
- `expect` and panics are allowed selectively where the code treats failure as unrecoverable or test-only; do not remove existing justified uses blindly.
- When parsing user or config input, produce explicit messages like `Failed to parse TOML` or `Invalid umask: ...`.

### Clippy Expectations

- This repo opts into aggressive clippy groups: `pedantic`, `nursery`, `cargo`, and `restriction`.
- `mise run check:clippy` is intentionally strict; do not suppress lints unless it is genuinely necessary and you can justify it clearly.
- Many specific lints are intentionally allowed in `Cargo.toml`; do not assume a triggered restriction lint means the pattern is always forbidden.
- Prefer targeted `#[expect(..., reason = "...")]` when a lint suppression is justified.
- Include a real reason string with each `#[expect]` or `#[allow]`.
- `std::assert_eq!` and `std::assert_ne!` are disallowed; use `pretty_assertions::assert_eq!` and `pretty_assertions::assert_ne!` in tests.

### Control Flow And Implementation Style

- Prefer straightforward, idiomatic Rust over clever abstractions.
- Small helper functions are preferred over large monoliths.
- Early returns are common and acceptable.
- Use iterator combinators when they improve clarity, but not at the expense of readability.
- Determinism matters: explicitly sort collections before order-sensitive iteration when needed.
- Preserve security-sensitive checks, especially around remote paths, symlinks, permissions, and shell quoting.

### Comments And Docs

- All items usually have doc comments; follow the existing style.
- Keep comments factual and local to the code.
- Add comments for non-obvious safety, security, or path-resolution behavior.

## Testing Conventions

- Unit tests usually live in `#[cfg(test)] mod tests` within the same file.
- Integration tests live in `tests/` and may share helpers through `tests/common/mod.rs`.
- Snapshot tests use `insta`.
- Always use `pretty_assertions` in tests for diff quality.
- For environment mutation in tests, keep `#[serial]` and cleanup guards.

## Generated And Sensitive Files

- Do not edit generated outputs directly when a render task exists.
- `biwa.toml` is gitignored local config and may contain secrets; do not copy its real values into committed docs or code.
- Avoid committing `.env` contents or values from local config files.

## Safe Agent Workflow

- Before editing, inspect nearby code and mirror the local style.
- After Rust edits, at minimum run focused tests plus `mise check:rustfmt`.
- Before finishing broader changes, prefer `LINT=true mise run check` and relevant tests.
- If you change generated CLI docs or schema inputs, run the corresponding `mise run render:*` task.
- If you change snapshots intentionally, run `mise run test:update-snapshot`.

## Good Defaults For Agents

- Prefer `mise run` tasks when a named task exists.
- Prefer minimal, surgical patches over broad refactors.
- Preserve tab-based formatting.
- Preserve existing logging, error-context, and lint-suppression patterns.
- When uncertain, align with the surrounding file rather than imposing generic style.

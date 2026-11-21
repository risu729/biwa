# Contributing

Thank you for your interest in contributing to biwa! This project is a volunteer effort and is **not associated with UNSW CSE**.

## Getting Started

The only requirements to contribute are **mise**:

1. Install **mise** (if you haven't already): [mise.jdx.dev](https://mise.jdx.dev)
2. Install dependencies:
   ```bash
   mise install
   ```

That's it! The environment is automatically managed by mise and `mise.toml`.

## Guidelines

- **Contributions Welcome**: We welcome all contributions, including bug fixes, features, and documentation improvements. Even small **typo fixes** are highly appreciated!
- **AI Contributions**: We allow AI-generated code, but **you must understand the changes**. Submitting blindly generated code that you cannot explain or debug is discouraged. Please verify that your AI-generated code works as intended.
- **Please be considerate**: Maintainers are volunteers. We might not have time to check every PR immediately, or fix every issue.
- **No guarantees**: We may not merge your PR if it doesn't align with the project's goals or quality standards.
- **Communication**: Opening an issue to discuss major changes before submitting a PR is recommended.

## Development Workflow

1. Fork the repository.
2. Create a feature branch.
3. Make your changes.
4. Run tests and linters:
   ```bash
   mise run test
   mise run check
   ```
5. Submit a Pull Request.

End-to-end SSH tests use the Docker Compose services in `docker-compose.yml`. `pitchfork`
normally starts them automatically; if they are not running, use:

```bash
pitchfork start --all
```

`pitchfork` assigns worktree-specific SSH ports when the defaults are already in use.
Run tests through `mise run test` so the resolved ports are passed to the end-to-end
test helpers.

## Testing

Start with the smallest test that covers the behavior you changed, then run the
full suite before opening a pull request:

```bash
# A unit test or module
cargo test cli_run_subcommand

# One SSH integration test
cargo test --test ssh_e2e_sync e2e_sync_empty_dir_created

# Keep test output while investigating a failure
cargo test e2e_sync_empty_dir_created -- --nocapture

# Full Rust suite and CI-like lint checks
mise run test
mise run check --lint
```

Run SSH end-to-end tests for behavior that depends on the server, SFTP, or remote
shell execution. For parser, configuration, and sync-planning changes, prefer a
focused unit test first. When adding coverage for a risk-prone path, generate a
local coverage report with `mise run test:coverage`; the HTML report is written to
`tarpaulin-report.html`.

Regenerate checked-in artifacts whenever their inputs change:

```bash
# CLI usage reference and configuration schema
mise run render:usage
mise run render:schema

# Intentional snapshot changes
mise run test:update-snapshot
```

`render:schema` caches the generated schema by source content, dependency locks,
resolved tools and Rust build flags. Repeated runs reuse the result, and mise
restores a deleted or edited `schema/config.json` from the local artifact cache.
Use
`mise run --force render:schema` to regenerate it unconditionally; tests and checks
continue to run normally.

## Documentation

To work on documentation:

```bash
mise run docs:dev
```

This starts a local development server where you can preview your changes.

## Documentation Deployment

GitHub Actions deploys the documentation Worker through
`risu729/wrangler-deploy-action`. Maintainers configure repository variable
`CLOUDFLARE_ACCOUNT_ID` and repository secret `CLOUDFLARE_API_TOKEN`.

The token needs `Individual Workers Editor` scoped only to the existing
`biwa-docs` Worker on account `risu`. The same permission supports production
version deployment and named Workers Preview creation.

On 2026-11-11 the owner selected routine production trigger reconciliation.
Production uses `deploy-triggers: true`: it uploads the validated artifact,
verifies that exact version at 100%, then synchronizes `biwa.takuk.me`,
`workersDev: false` and `previewUrls: true`. Preview builds still omit production
domains. The separate manual hostname workflow has been removed.

The shared action checks for domain conflicts before publication and immediately
before synchronization, then verifies the resulting attachment. Authentication
or verification failures fail the release; a failure after activation does not
roll back the published version. The checks are not an atomic lock against an
external writer, so retain the existing production concurrency.

Use the existing Worker-scoped credential and verify actual legacy endpoint
access. Add only demonstrated necessary grants; the public account-wide domain
API's permission table does not establish that broader script permissions are
needed. Effective credential and production synchronization verification remain
separate from offline CI. No new route, schedule or binding is declared.

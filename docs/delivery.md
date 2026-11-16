# Delivery

How changes reach `main`, releases and the docs site. The workflows under
`.github/workflows/` and `tasks.toml` are the authority; update this page in the
PR that changes them.

## Pull requests

- One PR per scope (owner, 2026-11-16 17:07 UTC).
- PR titles use conventional commits (`type(scope): description`), checked by the
  `Lint PR title` job; see [AGENTS.md](../AGENTS.md#commit-message-and-pr-title-conventions).

## Merging

A code PR needs an independent subagent review. After that review and a green
`CI Check` on the PR, the agent merges it itself, without waiting for the owner
(owner, 2026-11-16 17:07 UTC).

Plans and proposals get an independent subagent review before the owner sees them.

## Docs deployment

- Deploy docs through `risu729/wrangler-deploy-action` v3 and pinned cf.
- `docs:build` depends on the shared `docs:render` task, then packages the
  complete site with the Cloudflare Vite plugin. Production
  restores the validated artifact after CI checks pass, then reconciles configured
  triggers after verifying the deployed version.
- Same-repository PRs use named Workers Previews. After `docs:render`, the
  action runs `cf previews deploy`, which packages the site with Preview
  context, as Cloudflare documents; cf has no standalone Preview build. The
  CI job is required by `CI Check`, so a broken Preview build or deployment
  blocks merging. Fork PRs only run validation; runs without Preview
  credentials, such as Dependabot's, skip the Preview. Superseded PR runs are
  cancelled with the rest of CI, and the job checks that the PR is open at the
  tested head, so re-running an old run cannot replace the current Preview.
- Previews of closed PRs are not deleted. Cloudflare evicts the least recently
  deployed Preview once a Worker reaches its Preview limit, and this
  assets-only Worker has no storage bindings whose data would outlive the PR.
  A closed PR's Preview stays reachable only at its unadvertised Preview URL
  until it is evicted. Revisit cleanup if the Worker gains bindings or secrets.
- cf holds the API token while building a Preview, so build-time code from
  same-repository PRs, including dependency updates, can read it.
- `docs:artifact:pack` archives already-validated production output.
  `docs:artifact:restore` requires `EXPECTED_REVISION` and verifies checksums,
  revision, production mode, and Worker name without rebuilding. CI runs these
  with `--no-deps --skip-tools` after tools and dependencies are prepared.
- Preview builds omit production domains. Keep production and Preview output
  separate; never replace the archived production artifact with a Preview.
- Keep the token at `Individual Workers Editor` scoped only to `biwa-docs`
  on account `risu`. This covers production version deployment and named
  Preview creation. Routine production now enables trigger synchronization.
  Verify the same credential against the actual legacy Custom Domain endpoint;
  add only demonstrated necessary permissions. Public account-wide domain API
  permission tables do not establish the legacy endpoint's effective grants.
  The shared action checks domain conflicts before publication and verifies the
  resulting attachment. Existing docs domain and URL settings remain unchanged.

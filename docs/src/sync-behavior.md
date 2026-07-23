# Sync Behavior

Synchronize project files between the local sync root and the remote server.

By default, `biwa sync` pushes local files to the remote server. `biwa run` automatically runs that push sync before executing your command unless `sync.auto` is set to `false` in your configuration.

## Features

- **Smart Hashing**: Computes SHA-256 hash to only upload or download eligible modified/new files.
- **Directory Tracking**: Synchronizes directory presence as well as file contents, including empty directories.
- **Cleanup**: Push deletes remote files and directories that no longer exist locally. Pull deletes selected local files and directories that no longer exist remotely.
- **Ignore files & Standard Filters**: By default, standard filters are used (`.gitignore`, parent git ignores, git excludes). Hidden files (such as `.env`) are **not** ignored by default. You can use the custom `.biwaignore` file to ignore them.
- **Secure Permissions**: Enforces `0700` for newly created directories. Transferred files receive source permissions restricted by the configured remote umask (for example, with `0077`, `0644` becomes `0600` and `0755` becomes `0700`). Files skipped because their content already matches keep their existing destination permissions; round trips also preserve the pre-run local permissions.

If a directory still exists locally after its last file is removed, `biwa sync` will keep it on the remote side as an empty directory instead of deleting it.

## Sync Direction

### Push: local to remote

Push is the default direction:

```sh
biwa sync
```

The local sync root is the source of truth. Files are uploaded when they are missing remotely, have different hashes, or `--force` is passed. Remote files and directories that are no longer present locally are removed.

### Pull: remote to local

Use the dedicated `pull` command to mirror the remote project directory into the local sync root:

```sh
biwa pull
biwa pull --remote-dir "~/course-work/lab01"
biwa pull --sync-root ./lab01 --remote-dir "~/course-work/lab01"
```

Pull is deliberately destructive and must be opted into. The remote directory is the source of truth: files are downloaded when they are missing locally, have different hashes, or `--force` is passed; selected local entries missing remotely are deleted; and empty remote directories are created locally. Local directories are removed only when they are empty.

Pull requires the remote project directory to already exist. This avoids treating a mistyped remote path as an empty source and deleting local files.

Pull refuses remote symlink entries instead of following or recreating them. Local symlinks in the selected scope are removed rather than followed, and parent symlinks are rejected. Downloads are staged and checked against the inventoried SHA-256 digest before local changes begin.

Top-level names beginning with `.biwa-pull-stage-` are reserved for private local pull transactions. They are excluded from pushes and rejected on pulls.

Git administrative paths named `.git` are never transferred or deleted, including the `.git` file used by linked worktrees and submodules.

### Push, run, then pull

Use `biwa run --pull` when a remote command writes files that should come back to the local project:

```sh
biwa run --pull make generated
```

This resolves one local root and remote directory, pushes the project, runs the command, and pulls the remote result only after a successful exit. `--pull` implies the initial push even when `sync.auto` is disabled.

Use `--pull-always` when partial results should also be pulled after a confirmed nonzero or signal exit:

```sh
biwa run --pull-always ./generate-report
```

Neither mode pulls after a failed push or an SSH failure without a confirmed remote exit status. Pulling partial results with `--pull-always` does not mask a failed command: biwa still exits unsuccessfully. The final pull aborts if selected local files changed while the remote command was running, preserving those local edits. Pre-existing local symlinks are preserved during a round trip; a conflicting remote result is rejected.

## Sync Root

When `sync.sync_root` and `--sync-root` are not set, biwa uses the nearest Git root as the default sync root. This keeps `biwa run`, `biwa sync`, and `biwa pull` aligned to the same remote project directory when you invoke them from subdirectories of the same repository.

Set `sync.default_to_git_root = false` or pass `--sync-cwd` to use the current working directory as the default sync root instead.

## Target Filtering & Path Resolution

Biwa utilizes `globset` for specifying target exclusions and inclusions, supporting standard Unix-style glob matching syntax. The `exclude` array configuration is additive to the CLI `--exclude` flag.

### Supported Globset Syntax

- `?`: Matches any single character.
- `*`: Matches zero or more characters.
- `**`: Recursively matches directories. Useful for prefix (`**/foo`), suffix (`foo/**`), or infix (`foo/**/bar`).
- `{abc,def}`: Matches `abc` or `def`.
- `[ab]`: Matches character `a` or `b`.

### Path Resolution

- **Configuration `exclude`**: Globs are resolved relative to the configuration file's root directory.
- **CLI `--exclude` / `--include`**: Globs are resolved relative to your current working directory (CWD).

_Example_: running `biwa sync --exclude "tests/**"` from a subdirectory will correctly match and exclude the `tests` folder relative to that directory.

For pull, remote paths are matched against their corresponding local path under the sync root. `.gitignore`, `.ignore`, parent git ignores, git excludes, `.biwaignore`, config excludes, CLI excludes, and CLI includes all apply before a remote file is considered. Ignored or excluded remote files are not downloaded, and local files outside the selected include/exclude scope are not deleted. This is intentionally narrower than push cleanup, which compares the full remote project directory against the selected local state.

Ignore files are read from the local destination at the start of a pull. An ignore file that exists only on the remote side is downloaded like any other selected file, and its rules take effect on the next pull.

## Working Directory

When running `biwa run`, commands execute inside the synced project directory on the remote server, not in the home directory. This means `pwd` will output the synced project path (e.g. `~/.cache/biwa/projects/myproject-a1b2c3d4-deadbeef`).

If the synced directory does not exist (e.g. with `--skip-sync` on a fresh server), it will be created automatically before the command runs.

### Overriding with `--remote-dir`

Use `--remote-dir` (`-d`) to override the remote working directory for `biwa run`, `biwa sync`, and `biwa pull`:

```sh
# Run a command in the home directory (sync is automatically skipped)
biwa run -d "~" ls

# Sync to a custom remote path
biwa sync -d /tmp/my-project

# Pull from a custom remote path
biwa pull -d /tmp/my-project

# Run a command in /tmp (sync is automatically skipped)
biwa run -d /tmp ls

# Sync to a custom remote path and run a command there
biwa run -d /tmp/my-project --sync ls

# Push, run, and pull against the same custom remote path
biwa run -d /tmp/my-project --pull make generated
```

When used with `biwa sync` or `biwa pull`, `--remote-dir` replaces the automatically computed `remote_root + project_name` path.
To prevent accidental data overwrites when executing standard commands across different remote paths, **using `-d` with `biwa run` automatically disables project synchronization (`--skip-sync`)**. Pass `--sync`, `--pull`, or `--pull-always` to opt into the corresponding transfer workflow.

## Remote directory cleanup

Biwa stores active transfer targets before remote work begins so automatic cleanup cannot remove a directory that is in use, then refreshes the record after each successful `biwa sync`, `biwa pull`, and `biwa run` transfer. State is kept under your [XDG state directory](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) (for example `~/.local/state/biwa/connections.json` on Linux). A failed attempt can therefore leave its target recorded for later cleanup.

### Automatic cleanup after sync, pull, and run

When **`clean.auto`** is `true` (the default), biwa may start a **background** `biwa clean --auto` process after a successful `biwa sync`, `biwa pull`, or `biwa run`, as long as password authentication is not interactive-only (non-interactive auth such as env password, key, or agent is required). You can turn this off globally with **`BIWA_CLEAN_AUTO=false`** or in config:

```toml
[clean]
auto = false
```

Automatic cleanup connects over SSH, reads disk **quota** usage when the server reports it, and applies **`[clean]`** rules:

- **`max_age`** — Maximum age for remote directories under the default layout (`remote_root` + per-project folder names). Expressed as a duration string (`"30d"`, `"12h"`, `"30"` for 30 minutes, and so on). This acts as the baseline "0% quota" threshold.
- **`quota_thresholds`** — Optional map of quota usage **percentages** (0–100) to maximum directory ages. When reported quota usage is at or above a threshold, directories older than that threshold's age can be removed. If quota data is unavailable, only the baseline age from `max_age` applies.

Candidates for removal are **tracked** connection entries that are older than the effective age limit, plus **orphan** directories on the server that look like default biwa project folders under `remote_root`, are no longer listed in local state, and have a remote filesystem modification time older than the effective age limit. Orphan detection only runs when local state already has at least one tracked path for that SSH target, so it is not run on an empty or broken state file.

Background cleanup does not run if another cleanup process already holds the PID lock, or if interactive password auth would be required.

### Manual `biwa clean`

Use the dedicated command for explicit control; see the [CLI reference for `biwa clean`](/cli/clean.md).

- **Default (no extra flags)** — Remove the **current project's** remote directory (from the computed `remote_root` + unique project name for the current working directory).
- **`--all`** — Remove every **tracked** remote directory for this SSH host/user/port that matches the default biwa layout under `remote_root`.
- **`--purge`** — Remove every biwa-layout directory listed under `remote_root` on the server, including projects from other clients and legacy default-layout dirs. Use with care.
- **`--dry-run`** — Print what would be removed without deleting.
- **`biwa clean stop`** — Stop a running background cleanup daemon (if any).

Flags are resolved in a fixed order when combined: **`--auto`** (daemon-style quota cleanup) takes precedence over **`--purge`**, then **`--all`**, then the default current-project clean.

For destructive explicit clean invocations, biwa stops any running background cleanup daemon first so manual and automatic cleanup do not run at the same time.

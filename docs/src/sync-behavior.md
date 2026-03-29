# Sync Behavior

Synchronize local project files to the remote server.

By default, `biwa run` automatically runs `biwa sync` before executing your command unless `sync.auto` is set to `false` in your configuration.

## Features

- **Smart Hashing**: Computes SHA-256 hash to only upload modified/new files.
- **Directory Tracking**: Synchronizes directory presence as well as file contents, including empty directories.
- **Cleanup**: Automatically deletes remote files and directories that no longer exist locally.
- **Ignore files & Standard Filters**: By default, standard filters are used (`.gitignore`, parent git ignores, git excludes). Hidden files (such as `.env`) are **not** ignored by default. You can use the custom `.biwaignore` file to ignore them.
- **Secure Permissions**: Enforces `0700` for directories. File permissions are preserved from the local filesystem but restricted to user-only access (e.g. `0644` becomes `0600`, `0755` becomes `0700`).

If a directory still exists locally after its last file is removed, `biwa sync` will keep it on the remote side as an empty directory instead of deleting it.

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

## Working Directory

When running `biwa run`, commands execute inside the synced project directory on the remote server, not in the home directory. This means `pwd` will output the synced project path (e.g. `~/.cache/biwa/projects/myproject-a1b2c3d4`).

If the synced directory does not exist (e.g. with `--skip-sync` on a fresh server), it will be created automatically before the command runs.

### Overriding with `--remote-dir`

Use `--remote-dir` (`-d`) to override the remote working directory for both `biwa run` and `biwa sync`:

```sh
# Run a command in the home directory (sync is automatically skipped)
biwa run -d "~" ls

# Sync to a custom remote path
biwa sync -d /tmp/my-project

# Run a command in /tmp (sync is automatically skipped)
biwa run -d /tmp ls

# Sync to a custom remote path and run a command there
biwa run -d /tmp/my-project --sync ls
```

When used with `biwa sync`, `--remote-dir` replaces the automatically computed `remote_root + project_name` path.
To prevent accidental data overwrites when executing standard commands across different remote paths, **using `-d` with `biwa run` automatically disables project synchronization (`--skip-sync`)**. If you want to sync your project to a custom directory and run a command there in one step, you must explicitly pass the `--sync` flag.

## Remote directory cleanup

Biwa stores each successful `biwa sync` and `biwa run` (after sync, when applicable) in local state under your [XDG state directory](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) (for example `~/.local/state/biwa/connections.json` on Linux). That lets the tool know which remote project directories belong to this machine and when they were last used.

### Automatic cleanup after sync and run

When **`clean.auto`** is `true` (the default), biwa may start a **background** `biwa clean --auto` process after a successful `biwa sync` or `biwa run`, as long as password authentication is not interactive-only (non-interactive auth such as env password, key, or agent is required). You can turn this off globally with **`BIWA_CLEAN_AUTO=false`** or in config:

```toml
[clean]
auto = false
```

Automatic cleanup connects over SSH, reads disk **quota** usage when the server reports it, and applies **`[clean]`** rules:

- **`max_age`** — Maximum age for remote directories under the default layout (`remote_root` + per-project folder names). Expressed as a duration (`"30d"`, `"12h"`, a plain number for minutes, and so on). This acts as the baseline “0% quota” threshold.
- **`quota_thresholds`** — Optional map of quota usage **percentages** (0–100) to maximum directory ages. When reported quota usage is at or above a threshold, directories older than that threshold’s age can be removed. If quota data is unavailable, only the baseline age from `max_age` applies.

Candidates for removal are **tracked** connection entries that are older than the effective age limit, plus **orphan** directories on the server that look like default biwa project folders under `remote_root` but are no longer listed in local state—**only** when local state already has at least one tracked path for that SSH target (so orphan detection is not run on an empty or broken state file).

Background cleanup does not run if another cleanup process already holds the PID lock, or if interactive password auth would be required.

### Manual `biwa clean`

Use the dedicated command for explicit control; see the [CLI reference for `biwa clean`](/cli/clean.md).

- **Default (no extra flags)** — Remove the **current project’s** remote directory (from the computed `remote_root` + unique project name for the current working directory).
- **`--all`** — Remove every **tracked** remote directory for this SSH host/user/port that matches the default biwa layout under `remote_root`.
- **`--purge`** — Remove **all** directory entries listed under `remote_root` on the server (including projects from other clients). Use with care.
- **`--dry-run`** — Print what would be removed without deleting.
- **`biwa clean stop`** — Stop a running background cleanup daemon (if any).

Flags are resolved in a fixed order when combined: **`--auto`** (daemon-style quota cleanup) takes precedence over **`--purge`**, then **`--all`**, then the default current-project clean.

For explicit clean invocations, biwa stops any running background cleanup daemon first so manual and automatic cleanup do not run at the same time.

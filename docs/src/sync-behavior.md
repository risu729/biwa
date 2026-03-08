# Sync Behavior

Synchronize local project files to the remote server.

By default, `biwa run` automatically runs `biwa sync` before executing your command unless `sync.auto` is set to `false` in your configuration.

## Features

- **Smart Hashing**: Computes SHA-256 hash to only upload modified/new files.
- **Cleanup**: Automatically deletes remote files that no longer exist locally.
- **Ignore files & Standard Filters**: By default, standard filters are used (`.gitignore`, parent git ignores, git excludes). Hidden files (such as `.env`) are **not** ignored by default. You can use the custom `.biwaignore` file to ignore them.
- **Secure Permissions**: Enforces `0700` for directories. File permissions are preserved from the local filesystem but restricted to user-only access (e.g. `0644` becomes `0600`, `0755` becomes `0700`).

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

If the synced directory does not exist (e.g. with `--no-sync` on a fresh server), it will be created automatically before the command runs.

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
To prevent accidental data overwrites when executing standard commands across different remote paths, **using `-d` with `biwa run` automatically disables project synchronization (`--no-sync`)**. If you want to sync your project to a custom directory and run a command there in one step, you must explicitly pass the `--sync` flag.

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
- `{a,b}`: Matches `a` or `b`.
- `[ab]`: Matches character `a` or `b`.

### Path Resolution

- **Configuration `exclude`**: Globs are resolved relative to the configuration file's root directory.
- **CLI `--exclude` / `--include`**: Globs are resolved relative to your current working directory (CWD).

*Example*: running `biwa sync --exclude "tests/**"` from a subdirectory will correctly match and exclude the `tests` folder relative to that directory.

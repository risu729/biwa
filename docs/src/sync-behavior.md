# Sync Behavior

Synchronize local project files to the remote server.

By default, `biwa run` automatically runs `biwa sync` before executing your command unless `sync.auto` is set to `false` in your configuration.

## Features

- **Smart Hashing**: Computes SHA-256 hash to only upload modified/new files.
- **Cleanup**: Automatically deletes remote files that no longer exist locally.
- **Gitignore Support**: Respects `.gitignore` and `.ignore` files automatically.
- **Secure Permissions**: Enforces `0700` for directories. File permissions are preserved from the local filesystem but restricted to user-only access (e.g. `0644` becomes `0600`, `0755` becomes `0700`).

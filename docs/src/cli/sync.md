# `biwa sync`

The `biwa sync` command manually synchronizes your local project files to the remote server via SFTP.

```bash
biwa sync
# Alias
biwa s
```

> **Note:** By default, `biwa run` automatically runs `biwa sync` before executing your command unless `sync.auto` is set to `false` in your configuration.

## Features

### Smart Hashing
Biwa computes the SHA-256 hash of your local files and compares them against the remote state. It will only upload files that are newly created or whose contents have changed, making synchronization fast and avoiding unnecessary network transfers.

### Cleanup
During synchronization, Biwa calculates the delta between your local directory and the remote directory. Any files that exist on the remote server but no longer exist locally will be automatically deleted (cleaned up) from the remote directory.

### `.gitignore` Support
The synchronization process respects your project's `.gitignore` rules (as well as standard `.ignore` files), skipping files like `node_modules` or `target` automatically. You can also define custom ignore rules via `sync.ignore_files` in the configuration.

### Secure Permissions
To ensure the security of your sensitive project files, especially in shared environments (such as UNSW CSE servers), Biwa strictly enforces the following permissions on all synchronized resources:
- **Directories** are created with `0700` (`rwx------`) mode.
- **Files** are uploaded with `0600` (`rw-------`) mode.

These permissions guarantee that your code is accessible only to your user account.

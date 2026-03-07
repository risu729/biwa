# Configuration

`biwa` uses a layered configuration system, allowing you to define settings globally and override them locally per project.

::: warning Windows Not Supported
biwa does not run natively on Windows. Please use [WSL2](https://learn.microsoft.com/en-us/windows/wsl/install) (Windows Subsystem for Linux). All features work seamlessly inside WSL2.
:::

## Configuration File Locations

`biwa` looks for configuration files in the following order (later sources override earlier ones):

1.  **Global Configuration**:
    - `$HOME/biwa.<ext>`
    - `$HOME/.biwa.<ext>`
    - `$XDG_CONFIG_HOME/biwa/config.<ext>` (usually `$HOME/.config/biwa/config.<ext>`)

2.  **Local Configuration (Traversed)**:
    `biwa` searches from the current directory upwards, stopping before the home directory (which is handled as Global Configuration). Files found in deeper directories (closer to the current working directory) override those found in parent directories.
    - `./biwa.<ext>`
    - `./.biwa.<ext>`
    - `./.config/biwa.<ext>`

::: tip Relative Path Resolution
Any relative paths specified in your configuration (such as `ssh.key_path`) are resolved relative to **the project root** (for local configurations) or **your home directory** (for global configurations).

For example, if you set `key_path = "id_rsa"` in `./.config/biwa.toml`, it will look for the key at the project root `./id_rsa`, _not_ at `./.config/id_rsa`.
:::

3.  **Environment Variables**:
    - Any environment variable prefixed with `BIWA_`.
    - Nested keys use single underscores (e.g., `BIWA_SSH_HOST=myserver` maps to `ssh.host`).
    - Relative paths in environment variables are resolved relative to the **current working directory**.

## Supported Formats

`biwa` supports the following file extensions:

- `.toml` (Recommended)
- `.json`
- `.jsonc` / `.json5` (Both are parsed as JSON5, allowing comments and trailing commas)
- `.yaml` / `.yml`

## Configuration Reference

### `[ssh]` — SSH Connection Settings

| Key        | Type           | Default             | Description                                                                 |
| ---------- | -------------- | ------------------- | --------------------------------------------------------------------------- |
| `host`     | string         | `"cse.unsw.edu.au"` | SSH server hostname                                                         |
| `port`     | integer        | `22`                | SSH server port                                                             |
| `user`     | string         | `"z5555555"`        | Username (your zID)                                                         |
| `key_path` | string?        | `null`              | Path to SSH private key (auto-detected if not set)                          |
| `password` | bool \| string | `false`             | `false`: disabled, `true`: interactive prompt, `"string"`: literal password |

::: warning Password in Config
Storing your password in a configuration file is **not recommended** for security reasons. If you must use password authentication, prefer `password = true` for an interactive prompt or use environment variables (`BIWA_SSH_PASSWORD`).
:::

### `[log]` — Log Output Settings

| Key      | Type    | Default | Description                                                     |
| -------- | ------- | ------- | --------------------------------------------------------------- |
| `quiet`  | boolean | `false` | Suppress biwa internal logs, only showing remote command output |
| `silent` | boolean | `false` | Suppress all output, including remote command stdout/stderr     |

### `[sync]` — Synchronization Settings

| Key           | Type    | Default                                                | Description                                                                                 |
| ------------- | ------- | ------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `auto`        | boolean | `true`                                                 | Automatically synchronize the project before running remote commands                        |
| `sync_root`   | string? | `null`                                                 | Base directory to start the synchronization from. If not specified, uses current directory. |
| `engine`      | string  | `"sftp"`                                               | The synchronization engine to use (`"sftp"` or `"mutagen"`)                                 |
| `remote_root` | string  | `"~/.cache/biwa/projects"`                             | Remote directory to sync the project to                                                     |
| `exclude`     | array   | `["**/.git/**", "**/target/**", "**/node_modules/**"]` | List of target strings (using globset) to exclude during synchronization                    |

#### `[sync.sftp]` — SFTP Engine Settings

| Key                 | Type    | Default      | Description                                                               |
| ------------------- | ------- | ------------ | ------------------------------------------------------------------------- |
| `max_files_to_sync` | integer | `100`        | Abort synchronization if the number of files to upload exceeds this limit |
| `permissions`       | string  | `"recreate"` | Strategy for enforcing file permissions on uploaded files                 |

##### Permission Strategies

`biwa` ensures uploaded files have secure permissions (owner-only, no group/other access). Two strategies are available:

- **`recreate`** (default) — Deletes the remote file before re-creating it with the correct permissions set atomically at creation time. This is the most compatible strategy and works on all SFTP servers.

- **`setstat`** — Uses the SFTP `setstat` operation to set permissions after writing. This avoids deleting the file but **is not supported by all servers**. If `setstat` fails, biwa will log a warning suggesting you switch to `recreate`.

::: info SFTP Server Restrictions
Some SSH environments (notably UNSW CSE, which uses OpenSSH on networked filesystems) reject `setstat` / `fsetstat` SFTP operations with "Permission denied". If you see this error, ensure `sync.sftp.permissions` is set to `"recreate"` (the default).
:::

::: warning Absolute Remote Root
It is strongly recommended to use a relative path starting with `~` for your `remote_root`. Using an absolute path (e.g., `/home/user/cache`) can lead to unexpected directory structures and permissions issues on the remote server. Biwa will emit a warning if an absolute path is detected.
:::

## Schema Validation

`biwa` provides a JSON schema to enable autocompletion and validation in editors like VS Code.

To use the schema, add the following to your configuration file:

**TOML**:

```toml
#:schema https://biwa.takuk.me/schema/config.json

[ssh]
host = "cse.unsw.edu.au"
```

**JSON**:

```json
{
	"$schema": "https://biwa.takuk.me/schema/config.json",
	"ssh": {
		"host": "cse.unsw.edu.au"
	}
}
```

**YAML**:

```yaml
# yaml-language-server: $schema=https://biwa.takuk.me/schema/config.json
ssh:
  host: cse.unsw.edu.au
```

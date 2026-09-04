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

| Key                | Type    | Default             | Description                                                                                               |
| ------------------ | ------- | ------------------- | --------------------------------------------------------------------------------------------------------- |
| `host`             | string  | `"cse.unsw.edu.au"` | Hostname or OpenSSH `Host` alias                                                                          |
| `port`             | integer | OpenSSH, then `22`  | Biwa value, otherwise OpenSSH `Port`, otherwise `22`; duplicate values must match                         |
| `user`             | string? | OpenSSH `User`      | Optional direct username; required from either Biwa or OpenSSH config                                     |
| `use_ssh_config`   | boolean | `true`              | Read the supported subset of `~/.ssh/config`                                                              |
| `key_path`         | string? | `null`              | Explicit private key; disables automatic agent and default-key discovery                                  |
| `auth`             | string  | `"public-key"`      | Authentication mode: `"public-key"` or `"password"`                                                     |
| `host_key_checking`| string  | `"strict"`          | Host-key policy: `"strict"`, `"accept-new"`, or `"insecure"`                                           |
| `known_hosts`      | string? | `~/.ssh/known_hosts`| Optional known-hosts file override                                                                        |
| `umask`            | string  | `"077"`             | Umask (3-digit octal: owner/group/other) applied to the remote SSH execution environment and sync actions |

Biwa reads `Host`, `HostName`, `User`, `Port`, and `IdentityFile` from OpenSSH config. You may put `user`, `port`, and key selection in either Biwa or OpenSSH config. Equivalent duplicate values are accepted; conflicting values fail before connecting. `host` remains the lookup alias, while `HostName` supplies the network destination.

This is intentionally a subset of `ssh_config`. `Include`, `Match`, `IdentityAgent`, `IdentitiesOnly`, `PreferredAuthentications`, `PasswordAuthentication`, `KbdInteractiveAuthentication`, `ProxyCommand`, and `ProxyJump` behavior is not implemented. Some unsupported directives are ignored by `russh-config`; set `use_ssh_config = false` if parsing or partial support makes the result unsuitable.

::: tip Understanding `umask`
The `umask` setting ensures that any directories or files synced/created on the remote server maintain secure permissions (by default `077` prevents group and other access). Only the lower three digits are supported; to set the first digit (setuid/setgid/sticky), run `umask` manually on the remote server.

**Note that you cannot _loosen_ the default umask set by the server itself.** For example, the UNSW CSE server has a default umask of `027`. Even if you set biwa's umask to `022`, the server's restrictiveness will take precedence during file creation.

If you need looser permissions (e.g. making a file readable by others), you must manually run `chmod`. However, be aware that biwa's umask does not protect against manual `chmod` operations. If you mistakenly run `chmod +r` or `chmod +x` without restricting it to the user (e.g., `chmod u+x`), you might accidentally grant read/execute permissions to everyone.
:::

::: warning Password migration
The old `ssh.password` boolean/string field was removed. Select `auth = "password"` explicitly. Biwa prompts when interactive, or reads the secret from `BIWA_SSH_PASSWORD`; passwords are never accepted from configuration files and are never an automatic fallback from public keys.
:::

### Host key verification

`strict` is the secure default: the resolved hostname and port must already match `~/.ssh/known_hosts`. Connecting once with OpenSSH normally creates this entry. `accept-new` records an unknown key on first use but still rejects changed keys. Both policies reject a matching `@revoked` key. `insecure` accepts every key, emits a warning, and should be limited to isolated test environments.

```toml
[ssh]
host_key_checking = "accept-new"
# In a local config, this resolves from the project root.
known_hosts = ".biwa-known-hosts"
```

```bash
ssh-keyscan -p 22 cse.unsw.edu.au >> .biwa-known-hosts
```

Relative `known_hosts` paths follow the same rules described above: project root for local config, home directory for global config, and current directory for environment values. Verify a scanned key's fingerprint through a trusted channel before relying on it; `ssh-keyscan` alone does not authenticate the server.

### `[env]` — Environment Variable Settings

| Key              | Type           | Default    | Description                                              |
| ---------------- | -------------- | ---------- | -------------------------------------------------------- |
| `vars`           | array \| table | `[]`       | Environment variables to inherit, match, exclude, or set |
| `forward_method` | string         | `"export"` | Use `"export"` or `"setenv"` when sending variables      |

- Environment variable inheritance, wildcard matching, exclusions, and forwarding are documented in detail on [`/env-vars`](/env-vars).

### `[mise]` — mise Integration Settings

| Key              | Type    | Default  | Description                                                          |
| ---------------- | ------- | -------- | -------------------------------------------------------------------- |
| `enabled`        | boolean | `false`  | Run remote commands inside a [mise](https://mise.jdx.dev)-managed environment |
| `bin`            | string  | `"mise"` | mise executable on the remote host (bare name, absolute path, or `~`-relative path) |
| `mode`           | string  | `"exec"` | Wrapping strategy: `"exec"` or `"prefix"`                            |
| `env`            | string? | `null`   | mise environment name, forwarded to the remote command as `MISE_ENV` |
| `command_prefix` | string? | `null`   | Literal shell prefix used instead of the prefix built from `mode`    |
| `verify`         | boolean | `true`   | Check that the configured wrapper exists on the remote host before running a command |

The integration is off by default, so remote execution is unchanged until you
opt in. These settings are read only from global configuration or `BIWA_MISE_*`
environment variables:

```toml
# ~/biwa.toml (global configuration only)
[mise]
enabled = true
mode = "exec"
```

With that configuration, `biwa run node --version` executes
`mise exec -- node --version` in the remote project directory. See
[mise integration](/env-vars#mise-integration) for wrapping order, remote setup,
and the advanced `command_prefix` escape hatch.

The wrapper prefixes the command, so `biwa run 'a && b'` runs only `a` inside
the mise environment; use `biwa run sh -c 'a && b'` to run the whole compound
command under mise.

::: danger The `[mise]` section is global-only
`[mise]` selects the program that wraps every remote command, so biwa reads the
whole section only from global configuration or `BIWA_MISE_*` environment
variables and rejects it in project-local configuration. Otherwise a config file
committed to a cloned repository could choose what runs on your SSH host — with
`command_prefix`, or simply with `enabled` plus `bin`. See
[mise integration](/env-vars#modes).
:::

### `[direct]` — Direct Command Settings

| Key        | Type    | Default           | Description                                                |
| ---------- | ------- | ----------------- | ---------------------------------------------------------- |
| `bin_dir`  | string? | Platform data dir | Directory where `biwa activate install` creates shim links |
| `commands` | table   | `{}`              | Exact command names mapped to lists of `biwa run` options  |

These settings are read only from global configuration. Direct commands are
documented in detail on [`/direct-commands`](/direct-commands).

### `[sync]` — Synchronization Settings

| Key                   | Type    | Default                                                | Description                                                                   |
| --------------------- | ------- | ------------------------------------------------------ | ----------------------------------------------------------------------------- |
| `auto`                | boolean | `true`                                                 | Automatically synchronize the project before running remote commands          |
| `sync_root`           | string? | `null`                                                 | Base directory to start the synchronization from                              |
| `default_to_git_root` | boolean | `true`                                                 | Use the nearest Git root as the default sync root when `sync_root` is not set |
| `engine`              | string  | `"sftp"`                                               | The synchronization engine to use (`"sftp"` or `"mutagen"`)                   |
| `remote_root`         | string  | `"~/.cache/biwa/projects"`                             | Remote directory to sync the project to                                       |
| `exclude`             | array   | `["**/.git/**", "**/target/**", "**/node_modules/**"]` | List of target strings (using globset) to exclude during synchronization      |

#### `[sync.sftp]` — SFTP Engine Settings

| Key                 | Type    | Default      | Description                                                             |
| ------------------- | ------- | ------------ | ----------------------------------------------------------------------- |
| `max_files_to_sync` | integer | `100`        | Abort synchronization if the number of files to sync exceeds this limit |
| `permissions`       | string  | `"recreate"` | Strategy for enforcing file permissions on uploaded files               |

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

##### `[sync.sftp.cache]` — Sync Cache Settings

| Key       | Type    | Default            | Description                                                                     |
| --------- | ------- | ------------------ | ------------------------------------------------------------------------------- |
| `enabled` | boolean | `true`             | Reuse cached local file hashes while a file's metadata fingerprint is unchanged |
| `path`    | string? | State subdirectory | Directory to store sync cache files in                                          |

The sync cache speeds up repeated syncs by skipping re-hashing of local files whose size and modification time have not changed. See [Local hash cache](/sync-behavior#local-hash-cache) for how invalidation works and when to reset it.

### `[clean]` — Remote Directory Cleanup Settings

| Key                | Type    | Default | Description                                                                |
| ------------------ | ------- | ------- | -------------------------------------------------------------------------- |
| `max_age`          | string  | `"30d"` | Remove default-layout remote project directories older than this age       |
| `auto`             | boolean | `true`  | Start background cleanup after successful `biwa sync`, `biwa pull`, and `biwa run` calls |
| `quota_thresholds` | table   | `{}`    | Map quota usage percentages (`0`–`100`) to stricter maximum directory ages |

Duration values are strings such as `"30d"`, `"12h"`, `"45m"`, `"60s"`, or `"30"` for 30 minutes. `quota_thresholds` is merged with `max_age` as the baseline `0%` threshold; if quota data is unavailable, only `max_age` applies.

See [Remote directory cleanup](/sync-behavior#remote-directory-cleanup) for automatic cleanup behavior and manual `biwa clean` usage.

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

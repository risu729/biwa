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

3.  **Environment Variables**:
    - Any environment variable prefixed with `BIWA_`.
    - Use `__` (double underscore) to separate nested keys. (e.g., `BIWA_SSH__HOST=myserver` maps to `ssh.host`).

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
Storing your password in a configuration file is **not recommended** for security reasons. If you must use password authentication, prefer `password = true` for an interactive prompt or use environment variables (`BIWA_SSH__PASSWORD`).
:::

### `[log]` — Log Output Settings

| Key      | Type    | Default | Description                                                     |
| -------- | ------- | ------- | --------------------------------------------------------------- |
| `quiet`  | boolean | `false` | Suppress biwa internal logs, only showing remote command output |
| `silent` | boolean | `false` | Suppress all output, including remote command stdout/stderr     |

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

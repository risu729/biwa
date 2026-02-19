# Configuration

`biwa` uses a layered configuration system, allowing you to define settings globally and override them locally per project.

## Configuration File Locations

`biwa` looks for configuration files in the following order (later sources override earlier ones):

1.  **Global Configuration**:
    - `$HOME/biwa.<ext>`
    - `$HOME/.biwa.<ext>`
    - `$XDG_CONFIG_HOME/biwa/config.<ext>` (usually `~/.config/biwa/config.<ext>`)

    _Note: `XDG_CONFIG_HOME` locations take precedence over `$HOME` locations._

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

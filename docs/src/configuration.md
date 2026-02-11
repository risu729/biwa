# Configuration

Comprehensive guide to configuring biwa for your workflow.

## Configuration File

biwa supports **TOML**, **JSON**, **JSON5**, **JSONC**, **YAML**, and **YML** configuration files.

The configuration can be placed in:

- Project-specific: `biwa.toml`, `biwa.json`, etc. in your project root
- Global: `~/.config/biwa/config.toml` (or other formats)

Project-specific configuration takes precedence over global configuration.

## Initialization

Create a default configuration file using:

```bash
biwa init
```

This generates a configuration file (typically `biwa.toml`) in your project directory with default settings.

## Configuration Schema

### Remote Server Settings

```toml
[remote]
host = "cse.unsw.edu.au"      # Remote server hostname
user = "z5555555"               # Your username/zID
port = 22                       # SSH port (default: 22)
remote_root = "~/.cache/biwa"  # Remote directory for synced files
```

### SSH Settings

```toml
[ssh]
# Path to your SSH private key (optional, uses default if not specified)
ssh_key = "~/.ssh/id_ed25519"
# connect_timeout = 30               # Connection timeout in seconds
```

#### Password Authentication

If you don't provide an `ssh_key` and no SSH agent is active, biwa will prompt for a password.

::: warning Security
Password authentication works fine but is **not recommended** for frequent use due to security and convenience reasons. We strongly suggest setting up SSH keys.
:::

### Sync Configuration

```toml
[sync]
# Additional patterns to ignore (beyond .gitignore)
ignore_patterns = [
    "*.log",
    "tmp/",
    "node_modules/"
]

# Custom ignore file (like .gitignore but for biwa)
ignore_file = ".biwaignore"
```

### Environment Variables

```toml
[env]
# Simple key-value environment variables to transfer
vars = [
    "NODE_ENV",
    "DEBUG",
    "API_KEY"
]
```

::: warning Security Note
Be careful when transferring sensitive environment variables. Consider using mise or other secure secret management for production credentials.
:::

### Hooks

```toml
[hooks]
pre_sync = "bun install"          # Run before syncing files
post_sync = "echo 'Sync complete'" # Run after syncing
```

### Mise Integration

```toml
[mise]
# Load specific mise environment on remote
environment = "production"

# Prefix for all remote commands
command_prefix = "mise x --"
```

## Example Configurations

### Minimal Configuration

**biwa.toml**
```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"
```

**biwa.json**
```json
{
  "remote": {
    "host": "cse.unsw.edu.au",
    "user": "z5555555"
  }
}
```

### Course-Specific Configuration

For UNSW CSE course work:

```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"

[sync]
# Don't sync large test files or binaries
ignore_patterns = [
    "*.o",
    "*.out",
    "test_data/",
    "*.mp4"
]

[hooks]
# Ensure dependencies are installed before syncing
pre_sync = "make clean"
```

## Configuration Precedence

When biwa looks for configuration, it checks in this order:

1. Local config (`biwa.toml`, `biwa.json`, etc.)
2. Traverse up directories looking for config files
3. Global config (`~/.config/biwa/config.toml`, etc.)

## Schema Validation

biwa validates your configuration on startup. If there are issues, you'll see helpful error messages:

```bash
$ biwa run echo test
Error: Invalid configuration
  - remote.host is required
  - remote.user is required
```

## Next Steps

- Learn about [environment variable handling](/configuration#environment-variables)
- Explore [hooks](/configuration#hooks) for automation
- Set up [mise integration](/configuration#mise-integration) for advanced workflows

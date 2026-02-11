# Configuration

Comprehensive guide to configuring biwa for your workflow.

## Configuration File

biwa uses TOML configuration files. The configuration can be placed in:

- Project-specific: `.biwa.toml` or `biwa.toml` in your project root
- Global: `~/.config/biwa/config.toml`

Project-specific configuration takes precedence over global configuration.

## Initialization

Create a default configuration file using:

```bash
biwa init
```

This generates a configuration file with sensible defaults that you can customize.

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
identity_file = "~/.ssh/id_ed25519"  # Path to private key
# auth_method = "publickey"          # Authentication method
# connect_timeout = 30               # Connection timeout in seconds
```

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

For simple use cases:

```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"
```

### Advanced Configuration

For complex projects:

```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"
port = 22
remote_root = "~/.cache/biwa/projects/my-project"

[ssh]
identity_file = "~/.ssh/id_ed25519"

[sync]
ignore_patterns = [
    "*.log",
    ".venv/",
    "__pycache__/",
    "dist/",
    "build/"
]

[env]
vars = ["NODE_ENV", "DEBUG"]

[hooks]
pre_sync = "npm run build"

[mise]
environment = "dev"
command_prefix = "mise x --"
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

1. `.biwa.toml` in current directory
2. `biwa.toml` in current directory  
3. Traverse up directories looking for config files
4. `~/.config/biwa/config.toml` (global configuration)

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

## Configuration Tips

### Use Environment-Specific Configs

Keep different configs for different environments:

```bash
# Development
cp biwa.dev.toml .biwa.toml

# Production  
cp biwa.prod.toml .biwa.toml
```

### Version Control

Add to `.gitignore` if it contains sensitive data:

```gitignore
.biwa.toml
biwa.toml
```

Or use a template approach:

```bash
# Commit a template
git add biwa.toml.example

# Users copy and customize
cp biwa.toml.example .biwa.toml
```

### Share Global Config

For consistent settings across projects, use global config:

```bash
mkdir -p ~/.config/biwa
```bash
cat > ~/.config/biwa/config.toml << 'EOF'
[remote]
user = "z5555555"
host = "cse.unsw.edu.au"

[ssh]
identity_file = "~/.ssh/id_ed25519"
EOF
```

Then each project only needs project-specific settings.

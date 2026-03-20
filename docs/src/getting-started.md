# Getting Started

Get up and running with biwa in minutes.

::: warning Windows Not Supported
biwa does not run natively on Windows. Please use [WSL2](https://learn.microsoft.com/en-us/windows/wsl/install) (Windows Subsystem for Linux). All features work seamlessly inside WSL2.
:::

## Installation

### Via mise (Recommended)

First, install [mise](https://mise.jdx.dev/getting-started.html) if you haven't already. Then:

```bash
# Install biwa
mise use -g github:risu729/biwa

# Verify installation
biwa --version
```

### Via Cargo

If you have Rust installed, you can install biwa directly from crates.io:

```bash
cargo install biwa
```

### From Release Assets (Binary)

Download the latest release for your platform from the [Releases page](https://github.com/risu729/biwa/releases).

### From Source

For the latest development version:

```bash
git clone https://github.com/risu729/biwa.git
cd biwa
cargo install --path .
```

## Configuration

### Initialize Configuration

Run the initialization command to create a configuration file:

```bash
biwa init
```

This creates a `biwa.toml` file in your project directory with default settings.

### Basic Configuration

Edit the generated configuration file to add your CSE server details:

```toml
[ssh]
host = "cse.unsw.edu.au"
user = "z5555555"  # Your zID
port = 22

# Path to your SSH private key (optional, uses default if not specified)
# key_path = "~/.ssh/id_ed25519"
```

::: tip SSH Key Authentication
SSH key authentication is recommended over password authentication. See the [SSH Key Setup](/ssh-key-setup) guide for instructions.
:::

## First Run

Test your configuration:

```bash
# Run a simple command remotely
biwa run echo "Hello from CSE!"

# Run course-specific commands
biwa run 1511 autotest lab01
biwa run give cs1521 lab02
```

::: tip
If you're in a project directory, biwa will automatically sync your local files to the remote server before executing commands.
:::

## Log Output

By default, biwa shows internal logs (connection status, etc.) alongside remote command output. You can control this:

```bash
# Suppress biwa logs, only show remote output
biwa -q run echo "Hello"

# Suppress all output (including remote stdout/stderr)
biwa -s run echo "Hello"

# Increase verbosity for debugging
biwa -vv run echo "Hello"
```

You can also set `BIWA_LOG_QUIET=true` or `BIWA_LOG_SILENT=true` in the environment for the same behavior.

## Next Steps

- Read about [Configuration](/configuration) options
- Learn how env forwarding works in [Environment Variables](/env-vars)
- Set up [SSH Key Authentication](/ssh-key-setup)
- Check [About biwa](/about)
- Explore advanced features like hooks and mise integration

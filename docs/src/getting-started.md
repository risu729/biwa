# Getting Started

Get up and running with biwa in minutes.

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

This creates a configuration file (typically `biwa.toml` or `biwa.json`) in your project directory with default settings.

### Basic Configuration

Edit the generated configuration file to add your CSE server details:

```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"  # Your zID
port = 22

[ssh]
# Path to your SSH private key (optional, uses default if not specified)
# Using password authentication is possible but less secure
ssh_key = "~/.ssh/id_ed25519"
```

::: warning SSH Key Setup
Make sure you have SSH key authentication set up for CSE servers. Password authentication works but is less secure and inconvenient for frequent use.
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

## Next Steps

- Read about [Configuration](/configuration) options
- Check [About biwa](/about)
- Explore advanced features like hooks and mise integration

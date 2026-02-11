# Getting Started

Get up and running with biwa in minutes.

## Prerequisites

### Install mise (Recommended)

We strongly recommend using [mise](https://mise.jdx.dev/) to manage biwa and other development tools. mise makes it easy to install and keep tools up to date.

```bash
# Install mise (see https://mise.jdx.dev/getting-started.html)
curl https://mise.run | sh

# Activate mise in your shell
echo 'eval "$(mise activate bash)"' >> ~/.bashrc  # for bash
# or
echo 'eval "$(mise activate zsh)"' >> ~/.zshrc   # for zsh
```

### Using bun for Development

If you're developing biwa itself or working with the documentation, you'll need bun:

```bash
# Install bun via mise (recommended)
mise use -g bun@latest

# Or install directly
curl -fsSL https://bun.sh/install | bash
```

::: tip
For most users, you only need mise. Bun is only required if you're contributing to biwa's documentation or development.
:::

## Installation

### Via mise (Recommended)

```bash
# Install biwa
mise use -g biwa@latest

# Verify installation
biwa --version
```

### Via Cargo

If you prefer using Rust's package manager:

```bash
cargo install biwa
```

### From Source

For the latest development version:

```bash
git clone https://github.com/risu729/biwa.git
cd biwa
cargo build --release
# Binary will be in target/release/biwa
```

## Configuration

### Initialize Configuration

Run the initialization command to create a configuration file:

```bash
biwa init
```

This creates a configuration file (typically `biwa.toml` or `.biwa.toml`) in your project directory with default settings.

### Basic Configuration

Edit the generated configuration file to add your CSE server details:

```toml
[remote]
host = "cse.unsw.edu.au"
user = "z5555555"  # Your zID
port = 22

[ssh]
# Path to your SSH private key (optional, uses default if not specified)
identity_file = "~/.ssh/id_ed25519"
```

::: warning SSH Key Setup
Make sure you have SSH key authentication set up for CSE servers. Password authentication may work but is not recommended for regular use.
:::

### Setting Up SSH Keys

If you haven't set up SSH keys yet:

```bash
# Generate a new SSH key (if you don't have one)
ssh-keygen -t ed25519 -C "your.email@unsw.edu.au"

# Copy your public key to CSE server
ssh-copy-id z5555555@cse.unsw.edu.au
```

For more details on SSH configuration, see the [UNSW CSE documentation](https://www.cse.unsw.edu.au/).

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
- Check the [Comparison](/comparison) with other tools
- Explore advanced features like hooks and mise integration

## Troubleshooting

### Connection Issues

If you can't connect:

1. Verify SSH access works directly: `ssh z5555555@cse.unsw.edu.au`
2. Check your configuration file paths
3. Ensure your SSH key is properly set up

### Sync Issues

If files aren't syncing correctly:

1. Check your `.gitignore` - biwa respects ignored files
2. Review sync configuration in `biwa.toml`
3. Use verbose mode for debugging: `biwa run --verbose your-command`

## Getting Help

- GitHub Issues: [risu729/biwa/issues](https://github.com/risu729/biwa/issues)
- Documentation: Check other pages in this site
- CSE Resources: [UNSW CSE Help](https://www.cse.unsw.edu.au/help)

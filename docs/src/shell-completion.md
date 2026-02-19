# Shell Completion

biwa supports tab completion for bash, zsh, and fish shells.

## Prerequisites

Shell completions require the [`usage`](https://usage.jdx.dev) CLI to be installed.

Install it via [mise](https://mise.jdx.dev):

```bash
mise use -g usage
```

Or see the [usage installation docs](https://usage.jdx.dev/cli/) for other methods.

## Setup

We recommend using `eval` to load completions in your shell configuration. This ensures completions are always up to date with the installed version of biwa.

### Bash

Add to `~/.bashrc`:

```bash
eval "$(biwa completion bash)"
```

### Zsh

Add to `~/.zshrc`:

```zsh
eval "$(biwa completion zsh)"
```

### Fish

Add to `~/.config/fish/config.fish`:

```fish
biwa completion fish | source
```

## Verify

After restarting your shell (or sourcing the config), try:

```bash
biwa [TAB]
```

You should see available subcommands like `run`, `init`, `completion`, etc.

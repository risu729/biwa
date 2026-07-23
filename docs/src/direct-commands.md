# Direct Commands

Direct commands let selected command names dispatch through `biwa` without typing `biwa run`.
For example:

```bash
1511 autotest lab01
```

is handled as `biwa run 1511 autotest lab01`, using the same configuration,
synchronization, and remote execution path.

## Configuration

Define each command in your **global** biwa configuration:

```toml
[direct]
# Optional; defaults to the platform data directory.
bin_dir = "~/.local/share/biwa/bin"

[direct.commands]
"1511" = []
dcc = ["--skip-sync", "--remote-dir", "~/dcc"]
```

The table key is the exact command name. Its value is a list of `biwa run` options
inserted before that command. An empty list uses the normal run defaults.

Direct command settings are intentionally global-only. Project-local
`direct.commands` entries are ignored when selecting a shim, but the resulting
`biwa run` still loads the normal project and global configuration for SSH,
synchronization, environment variables, and hooks.

Command names may contain ASCII letters, digits, `-`, `_`, `.`, and `+`, and may
not begin with `-`. The names `.`, `..`, `biwa`, `biwa.exe`, and the internal
`.biwa-*` namespace are reserved.

## Install Shims

Reconcile the configured shims:

```bash
biwa activate install
```

This creates one symlink per `direct.commands` entry, updates symlinks whose biwa
target changed, and uses a manifest in the shim directory to remove only stale
symlinks previously created by biwa. Existing non-symlink files and untracked
symlinks are preserved; use `--force` to replace an existing untracked entry
whose name is explicitly configured.

The links point directly to the `biwa` executable. When invoked through a link,
biwa reads its executable name and expands the invocation into the equivalent
`biwa run` arguments.

## Activate Your Shell

Add the appropriate command to your shell configuration:

### Bash

```bash
eval "$(biwa activate --shell bash)"
```

### Zsh

```zsh
eval "$(biwa activate --shell zsh)"
```

### Fish

```fish
biwa activate --shell fish | source
```

Activation removes earlier occurrences of the shim directory and appends it once
to `PATH`, so an existing local executable wins when it appears earlier. Run
`biwa activate doctor` to show the global shim directory and configured commands.

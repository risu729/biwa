# Direct Commands

Direct commands let selected command names dispatch through `biwa` without typing `biwa run`.

For example, after activation, a command like this:

```bash
1511 autotest lab01
```

runs the allowed remote command `1511 autotest lab01` using the same synchronization and remote execution path as `biwa run`.

## Configuration

Direct commands are disabled by default. Enable them and allow only the command names you want to run remotely:

```toml
[direct]
enabled = true
bin_dir = "~/.local/share/biwa/bin"
allow = ["^\\d{4}$", "^(give|autotest|dcc|1521)$"]
default_args = { "1511" = ["--course", "comp1511"] }
prefer_local = true
```

- `enabled` must be `true` before shim invocations dispatch remotely.
- `bin_dir` is the directory added to your shell `PATH`.
- `allow` contains regular expressions matched against the shim command name.
- `default_args` inserts configured arguments after the command name and before arguments typed at the shell.
- `prefer_local` keeps existing local commands earlier in `PATH` ahead of biwa shims.

## Install Shims

Create or update static command shims:

```bash
biwa activate install
```

`biwa` can create shims for literal allow entries such as `^dcc$`, simple alternatives such as `^(give|autotest)$`, and keys present in `direct.default_args`. Regex families such as `^\\d{4}$` are matched at runtime, but `activate install` cannot enumerate every possible name from them; add the specific command as a `default_args` key when you want a static shim for it.

## Activate Your Shell

Add one of these to your shell configuration:

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

Run diagnostics with:

```bash
biwa activate doctor
```

## Conflict Behavior

When `direct.prefer_local = true`, `biwa activate install` skips a shim if an executable with the same name appears earlier in `PATH`. The message identifies the local command that would take precedence. Use `biwa activate install --force` to create configured shims anyway and replace existing files in the shim directory.

To replace a shim, rerun `biwa activate install`. To remove direct command support, remove the activation line from your shell config and delete the shim directory:

```bash
rm -rf ~/.local/share/biwa/bin
```

Only command names matched by `direct.allow` dispatch remotely. Unknown shim names fail instead of turning arbitrary local commands into remote commands.

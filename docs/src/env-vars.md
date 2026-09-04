# Environment Variables

`biwa` can forward local environment variables to the remote process (inheritance), send explicit values, and expand wildcard rules.

## Config Keys

| Key                  | Type          | Default    | Description                                              |
| -------------------- | ------------- | ---------- | -------------------------------------------------------- |
| `env.vars`           | array / table | `[]`       | Environment variables to inherit, match, exclude, or set |
| `env.forward_method` | string        | `"export"` | Use `"export"` or `"setenv"` when sending variables      |

## Supported Config Forms

### Array

```toml
[env]
vars = ["NODE_ENV", "API_KEY=secret", { DEBUG = "1" }]
forward_method = "export"
```

### Table

```toml
[env]
forward_method = "export"

[env.vars]
NODE_ENV = true
API_KEY = "secret"
```

### Array Of Inline Tables

```toml
[env]
vars = [{ NODE_ENV = "production" }, { API_KEY = "secret" }]
```

- `NAME` or `NAME = true` inherits the local value from your machine to the remote process.
- `NAME=value` or `NAME = "value"` sends a literal value.

## Wildcards And Negation

All `env.vars` forms (array, table, array of tables) support wildcard rules:

```toml
[env]
vars = ["NODE_*", "!*PATH"]
```

- `*` matches zero or more characters in an environment variable name.
- `NODE_*` inherits all local variables whose names start with `NODE_`.
- `!*PATH` removes already-selected variables whose names end in `PATH`.
- Prefer targeted patterns like `NODE_*`, `AWS_*`, or `CARGO_*`.
- Avoid mixing catch-all `*` with explicit variable names in the same `env.vars` section; if you need broad matching, use specific prefixes plus exclusions instead.

### Evaluation Order

Regardless of the config form or declaration order, rules are always evaluated deterministically:

1. **Inherit patterns** — wildcard matches like `NODE_* = true` expand first.
2. **Exact specifications** — explicit names like `NODE_ENV = true` or `API_KEY = "secret"` override inherited values.
3. **Exclusions** — removal rules like `!*PATH = true` apply last.

This means an explicit value always takes priority over a pattern-inherited one. For example, with `NODE_* = true` and `NODE_ENV = "prod"`, even if the local machine has `NODE_ENV = "dev"`, the result will be `NODE_ENV = "prod"`.

## `BIWA_ENV_VARS`

You can add environment variables from the local shell without touching config:

```bash
BIWA_ENV_VARS=NODE_ENV biwa run --skip-sync env
BIWA_ENV_VARS=NODE_ENV=prod biwa run --skip-sync env
BIWA_ENV_VARS=NODE_* biwa run --skip-sync env
```

- `BIWA_ENV_VARS=NODE_ENV` inherits a local value.
- `BIWA_ENV_VARS=NODE_ENV=prod` sets a literal value.
- `BIWA_ENV_VARS=NODE_*` uses wildcard inheritance.

## `biwa run --env`

`biwa run` supports repeated flags, such as names, wildcards, and `KEY=value` pairs:

```bash
biwa run --env NODE_ENV --env API_KEY env
biwa run --env NODE_ENV=prod --env API_KEY env
biwa run --env NODE_* --env '!*PATH' env
```

CLI `--env` values override config-defined env vars with the same name.

## Forwarding Methods

- `export` prepends shell-safe `export KEY=VALUE` statements to the remote command. This is the default and most compatible mode.
- `setenv` uses SSH `setenv` requests before running the command.

::: warning UNSW CSE
UNSW CSE does not support SSH `setenv`, so use `env.forward_method = "export"` there.
:::

## mise Integration

Direct env forwarding sends values from your local machine. If the remote host
should instead resolve tools and variables itself — from a project-local
`mise.toml` / `.tool-versions` — enable the [mise](https://mise.jdx.dev)
integration and biwa wraps each remote command with mise:

```toml
[mise]
enabled = true
mode = "exec"
```

```bash
biwa run node --version
biwa run bun test
biwa run --env NODE_ENV=production npm run build
```

Use direct forwarding for values that only exist on your machine (secrets, CI
tokens, per-run overrides), and mise for tool versions and project-wide
variables that belong to the remote checkout.

### Wrapping Order

Remote commands are always assembled in this order:

```sh
umask 077 && mkdir -p -- <remote dir> && cd <remote dir> && export NODE_ENV=production && mise exec -- npm run build
```

1. `umask` from `ssh.umask`.
2. `mkdir -p` and `cd` into the remote project directory, so mise discovers the
   synced project's `mise.toml`.
3. Environment forwarding (`export` statements, or SSH `setenv` requests before
   the command when `env.forward_method = "setenv"`).
4. `export MISE_ENV=<mise.env>`, when `mise.env` is set.
5. The mise wrapper.
6. Your command with shell-quoted arguments.

Forwarded variables are therefore visible to mise itself and are inherited by
the command it runs; explicit `mise.toml` values win over the forwarded ones
inside the mise environment.

### Modes

- `exec` (default) — prefixes the command with `<mise.bin> exec --`.
- `prefix` — prefixes the command with `mise.command_prefix` verbatim. Setting
  `command_prefix` also overrides the prefix in `exec` mode, so it works as a
  general escape hatch:

  ```toml
  [mise]
  enabled = true
  command_prefix = "mise x --"
  ```

  `command_prefix` is inserted without shell quoting, so treat it as trusted
  configuration. `mode = "prefix"` without a `command_prefix` is a
  configuration error.

The wrapper prefixes the whole command, exactly like running `env` or `nice` in
front of it, so shell operators inside a command string (`|`, `&&`, redirects)
still bind in the remote shell rather than inside mise.

::: info Shell activation
`mode = "activate"` (`eval "$(mise activate ...)"`) is not implemented. Remote
commands run through a non-interactive shell where activation hooks do not
fire reliably; `exec` provides the same tool and environment resolution without
depending on shell integration.
:::

### Remote Setup

mise has to exist on the remote host. Bootstrap it once:

```bash
biwa run --skip-sync 'curl https://mise.run | sh'
biwa run --skip-sync 'mise --version'
```

Before wrapping a command, biwa checks that `mise.bin` is available remotely and
fails with setup instructions when it is not. If the remote installation is not
on the non-interactive `PATH`, set `bin` to its absolute path (for example
`bin = "~/.local/bin/mise"`). Set `verify = false` to skip the extra check and
its round trip.

## Environment-Dependent Variables

biwa warns when you inherit machine-specific variables such as:

- `PATH`, `LD_LIBRARY_PATH`, `LIBRARY_PATH`
- `HOME`, `PWD`, `OLDPWD`
- `PYTHONHOME`, `PYTHONPATH`, `VIRTUAL_ENV`, `CONDA_PREFIX`
- `NODE_PATH`, `NPM_CONFIG_PREFIX`
- `JAVA_HOME`, `CLASSPATH`
- `GOPATH`, `GOBIN`, `GOMODCACHE`
- `GEM_HOME`, `GEM_PATH`, `BUNDLE_PATH`, `BUNDLE_BIN`
- `CARGO_HOME`, `RUSTUP_HOME`
- `PHP_INI_SCAN_DIR`

Those values often differ between your local machine and the remote host.

## Security

Inherited variables are injected into the remote process environment. Be careful when sending secrets, and prefer only the variables you actually need.

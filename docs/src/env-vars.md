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

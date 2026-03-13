# Environment Variables

`biwa` can forward local environment variables to the remote process (inheritance), or send explicit values.

## Config Keys

| Key                   | Type          | Default    | Description                                         |
| --------------------- | ------------- | ---------- | --------------------------------------------------- |
| `env.vars`            | array / table | `[]`       | Environment variables to inherit or set            |
| `env.forward_method`  | string        | `"export"` | Use `"export"` or `"setenv"` when sending variables |

## Supported Config Forms

### Array

```toml
[env]
vars = ["NODE_ENV", "API_KEY=secret", { DEBUG = "1" }]
transfer_method = "export"
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

## `BIWA_ENV_VARS`

You can add environment variables from the local shell without touching config:

```bash
BIWA_ENV_VARS=NODE_ENV,API_KEY biwa run --skip-sync env
BIWA_ENV_VARS=NODE_ENV=prod,OTHER_ENV biwa run --skip-sync env
```

- `BIWA_ENV_VARS=NODE_ENV,API_KEY` inherits local values.
- `BIWA_ENV_VARS=NODE_ENV=prod,OTHER_ENV` mixes literal values and inheritance.

## `biwa run --env`

`biwa run` supports repeated flags, comma-separated names, and `KEY=value` pairs:

```bash
biwa run --env NODE_ENV,API_KEY env
biwa run --env NODE_ENV=prod --env API_KEY env
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

Those values often differ between your local machine and the remote host.

## Security

Inherited variables are injected into the remote process environment. Be careful when sending secrets, and prefer only the variables you actually need.

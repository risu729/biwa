---
title: SSH Key Setup
order: 4
---

# Setting up SSH Key Authentication

SSH key authentication is the recommended way to connect to CSE servers. It's more secure and convenient than password authentication.

For the official CSE documentation, see: [SSH Keys — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/SSH_Keys/)

## Automated Setup

`biwa setup-ssh` performs the whole migration from password authentication for you:

```bash
biwa init
biwa setup-ssh
biwa run --skip-sync hostname
```

The command resolves the host, user, and port from your normal configuration, then:

1. Tries the credentials biwa normally uses — an SSH agent identity, an `IdentityFile` entry, `ssh.key_path`, or a standard key — and stops there if key authentication already works.
2. Otherwise selects `--key-path`, `ssh.key_path`, or the first existing standard key (`~/.ssh/id_ed25519`, then `~/.ssh/id_rsa`).
3. Offers to generate a new Ed25519 key pair when none exists, prompting for an optional passphrase.
4. Connects once with password authentication, prompting for the password interactively.
5. Creates `~/.ssh` (mode `700`) remotely and appends the public key to `~/.ssh/authorized_keys` (mode `600`) only if it is not already authorized there.
6. Opens a fresh connection with the key to confirm the result.

A local problem — an unreadable key, a companion `.pub` file that does not match its private key, or a `ssh.key_path` that disagrees with your OpenSSH `IdentityFile` — stops the command before anything remote is changed. When the key had just been generated, it is left on disk unused, and the message names it so you can remove it.

Step 1 offers every credential biwa would normally try, so a loaded agent can produce several rejected attempts before the command falls back to your password. A hardened server may throttle that; select the key you want with `--key-path` to try exactly one.

```bash
# Reuse or create a specific key
biwa setup-ssh --key-path ~/.ssh/id_ed25519

# Create the key when the path does not exist
biwa setup-ssh --generate --key-type ed25519

# Verify key authentication without changing anything
biwa setup-ssh --check

# Also write ssh.key_path into the nearest biwa configuration file
biwa setup-ssh --write-config
```

`--check` exits successfully whenever key authentication works, including through an agent, and fails otherwise. It never connects with a password and never changes local or remote files.

`--write-config` rewrites only the affected lines of the nearest TOML configuration file, keeping comments and formatting, and also switches `ssh.auth = "password"` to `"public-key"`. It verifies that every unrelated configuration value stays unchanged before writing. If the file cannot be updated safely, biwa prints the snippet to apply by hand; the key setup itself still succeeds.

The command is idempotent: re-running it never adds a duplicate `authorized_keys` entry, and it never prints private key material or your password. An entry counts as already authorized only when the line starts with the key itself, so a commented-out entry or one carrying options such as `from="..."` does not hide a missing authorization. Only the first line of a `.pub` file is installed, so a file holding several keys authorizes just the selected one.

::: tip Non-Interactive Use
Without a terminal, pass `--generate` to allow key creation and supply the password through `BIWA_SSH_PASSWORD`. A key generated without a terminal has no passphrase.
:::

::: warning POSIX Shell Required
The remote setup step runs a small POSIX shell script, like the rest of biwa's remote commands. If your login shell on the server is `csh`, `tcsh`, or `fish`, install the key manually as described below.
:::

::: warning Windows
Run `biwa setup-ssh` inside [WSL2](https://learn.microsoft.com/en-us/windows/wsl/install). Key paths are resolved on the machine biwa runs on, so a key created in WSL2 lives in the WSL2 home directory, not in `C:\Users\...\.ssh`.
:::

`--generate` creates Ed25519 keys only. To use an RSA key, create it with `ssh-keygen -t rsa` and pass it with `--key-path`.

If you prefer to control key setup yourself, the manual steps below remain fully supported.

## Generate an SSH Key

You can generate a key either **on your local machine** or **on the CSE server**.

### Option A: Generate Locally (Recommended)

```bash
ssh-keygen -t ed25519 -C "your_zid@unsw.edu.au"
```

Press Enter to accept the default file location (`~/.ssh/id_ed25519`). Set a secure passphrase when prompted.

### Option B: Generate on CSE Server

Connect to a CSE login server and run:

```bash
ssh-keygen -t rsa
```

Accept the defaults and set a passphrase. Then download the private key (`~/.ssh/id_rsa`) to your local machine.

::: tip Ed25519 vs RSA
Ed25519 keys are smaller and more secure. The CSE FAQ recommends RSA, and both work. biwa checks for Ed25519 first, then RSA.
:::

## Install Your Public Key on CSE

CSE doesn't support `ssh-copy-id`. Either run `biwa setup-ssh` as shown above, or add your public key manually:

```bash
# From your local machine, copy and append the public key
cat ~/.ssh/id_ed25519.pub | ssh z5555555@cse.unsw.edu.au 'cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys'
```

Or, if you generated the key on the CSE server, the public key is already there — just ensure:

```bash
chmod 600 ~/.ssh/authorized_keys
```

## Verify

```bash
ssh z5555555@cse.unsw.edu.au echo "Success!"
```

If this prints "Success!" without asking for a password, key auth is working. `biwa setup-ssh --check` performs the same verification through biwa's own configuration.

## How biwa Resolves Authentication

Public-key authentication is the default. No Biwa authentication setting is required. For automatic discovery, Biwa tries concrete credentials in this order:

1. Agent identities matching OpenSSH `IdentityFile` entries, in configured order
2. Private keys named by `IdentityFile`
3. Remaining agent identities, preserving agent order
4. Existing `~/.ssh/id_ed25519` and `~/.ssh/id_rsa` files

Every selected, deduplicated agent identity gets a fresh SSH connection, avoiding one connection's `MaxAuthTries` budget. A plain public key and an OpenSSH certificate for that key remain separate identities, so either can authenticate. Matching identities are all tried; unrelated agent identities are limited to 10. If an agent exposes more, add an `IdentityFile` public-key hint.

Setting `ssh.key_path` selects only that private key. It does not fall back to unrelated agent or default keys. Encrypted private keys prompt for a passphrase only when reached and only in an interactive process.

::: tip Zero-Config Users
If your key is at a standard path or loaded into the agent named by `SSH_AUTH_SOCK`, omit authentication settings. Biwa discovers it automatically and never falls back to a password prompt.
:::

## OpenSSH aliases and per-host agent selection

Biwa supports `Host`, `HostName`, `User`, `Port`, and `IdentityFile` from `~/.ssh/config`. This lets OpenSSH and Biwa share the destination and key hint:

```sshconfig
Host cse
    HostName cse.unsw.edu.au
    User z5555555
    IdentityFile ~/.ssh/cse.pub
```

```toml
[ssh]
host = "cse"
```

`IdentityFile` may point to a public-key file. Biwa compares its key bytes with identities exposed by Bitwarden, 1Password, OpenSSH `ssh-agent`, or another compatible agent. The private key does not need to exist on disk. A public key contains no secret and can be stored in dotfiles if that suits your setup.

`IdentitiesOnly` and `IdentityAgent` are not supported yet. Biwa uses `SSH_AUTH_SOCK`, and after configured matches it may try up to 10 unrelated agent identities.

## Explicit private key

To use a non-default key path:

```toml
[ssh]
user = "z5555555"
key_path = "~/.ssh/my_custom_key"
```

If `key_path` and OpenSSH `IdentityFile` are both present, they must select the same public-key bytes and `IdentityFile` must contain exactly one entry. Different or ambiguous selections are configuration errors.

## Password authentication

Password authentication is opt-in and never follows a failed public key automatically:

```toml
[ssh]
host = "cse"
auth = "password"
```

Biwa prompts once in an interactive terminal. For CI or another non-interactive process, provide the secret only through the environment:

```bash
BIWA_SSH_AUTH=password BIWA_SSH_PASSWORD='...' biwa run --skip-sync true
```

`ssh.password` was removed, and literal passwords are not accepted in configuration files. Environment variables can be visible to other processes on some platforms, so use your CI secret facility and keep logs redacted.

## Host key verification

Biwa defaults to strict host-key verification. Running the OpenSSH verification command above normally records the server in `~/.ssh/known_hosts`. An unknown, changed, or explicitly `@revoked` key stops before authentication. A revoked key is also rejected in `accept-new` mode rather than being learned again.

For trust on first use on a private host:

```toml
[ssh]
host_key_checking = "accept-new"
```

`insecure` disables verification and is intended only for isolated tests.

## Windows Users

::: warning WSL2 Recommended
If you're on Windows, we recommend using [WSL2](https://learn.microsoft.com/en-us/windows/wsl/install). SSH key management and agent forwarding work seamlessly in WSL2.
:::

## Troubleshooting

### Permission Denied

Make sure your key file permissions are correct:

```bash
chmod 700 ~/.ssh
chmod 600 ~/.ssh/id_ed25519
chmod 644 ~/.ssh/id_ed25519.pub
```

### Agent Not Working

Ensure your SSH agent is running and has your key loaded:

```bash
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519
```

Check that `SSH_AUTH_SOCK` points to the intended agent. For Bitwarden, enable its SSH agent integration and configure your shell to use the socket it provides. If many identities are available, put the matching public key in `IdentityFile` as shown above.

### OpenSSH config parsing fails

Biwa supports only the subset listed above. `Include` and `Match` are not fully evaluated, and proxy/authentication directives are not executed. As an escape hatch, configure all required connection values directly and disable reading OpenSSH config:

```toml
[ssh]
host = "cse.unsw.edu.au"
user = "z5555555"
use_ssh_config = false
```

## Further Reading

- [SSH Keys — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/SSH_Keys/) — Official UNSW CSE documentation
- [Logging In With SSH — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/Logging_In_With_SSH/) — How to connect to CSE servers

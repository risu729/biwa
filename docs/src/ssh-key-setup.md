---
title: SSH Key Setup
order: 4
---

# Setting up SSH Key Authentication

SSH key authentication is the recommended way to connect to CSE servers. It's more secure and convenient than password authentication.

For the official CSE documentation, see: [SSH Keys — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/SSH_Keys/)

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

CSE doesn't support `ssh-copy-id`. You need to manually add your public key:

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

If this prints "Success!" without asking for a password, key auth is working.

## How biwa Resolves Authentication

biwa tries authentication methods in this order. **Explicit configuration is always respected first:**

1. **Configured key file** — If `ssh.key_path` is set, biwa uses it (errors if not found)
2. **Configured password** — If `ssh.password` is a string, biwa uses it; if `true`, biwa prompts interactively
3. **Default key files** — biwa checks `~/.ssh/id_ed25519`, then `~/.ssh/id_rsa`
4. **SSH Agent** — If nothing else is configured and no keys found, biwa falls back to the SSH agent

::: tip Zero-Config Users
If you want to delegate authentication to your SSH agent, **don't configure any auth settings**. biwa will automatically use the agent as a fallback.
:::

## Configuration

To use a non-default key path:

```toml
[ssh]
user = "z5555555"
key_path = "~/.ssh/my_custom_key"
```

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

## Further Reading

- [SSH Keys — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/SSH_Keys/) — Official UNSW CSE documentation
- [Logging In With SSH — CSE FAQ](https://taggi.cse.unsw.edu.au/FAQ/Logging_In_With_SSH/) — How to connect to CSE servers

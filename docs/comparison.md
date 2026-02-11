# Comparison with Alternatives

When working with remote UNSW CSE servers, you have several options. Here's how biwa compares to the most common alternatives.

## VSCode Remote SSH

**VSCode Remote SSH** is a popular extension that allows you to use VS Code as if it were installed on the remote machine.

### Pros
- Full IDE experience on remote system
- Integrated terminal, debugging, and extensions
- Good for long development sessions

### Cons
- Heavy resource usage on remote server
- Requires VSCode specifically
- Can be slow over unstable connections
- Limited to graphical workflow

### biwa Advantage
biwa is lighter weight and editor-agnostic. You can use your preferred editor locally while leveraging remote compute only when needed. Commands execute quickly without the overhead of maintaining a persistent IDE connection.

---

## SSH FS (SSHFS)

**SSH FS** mounts remote directories as local filesystems, allowing you to edit files locally while they're stored remotely.

### Pros
- Edit files with any local editor
- Transparent file access
- No special client needed

### Cons
- Performance depends heavily on network latency
- Each file operation requires network round-trip
- Can feel sluggish for large projects
- Mounting/unmounting can be cumbersome

### biwa Advantage
biwa synchronizes files efficiently using rsync, providing local-speed file access for editing while running commands remotely. No mounting required, and you get the benefits of local file performance.

---

## Raw SSH Command Line

Using **SSH directly** is the traditional approach: `ssh user@cse.server` then running commands manually.

### Pros
- Direct control and transparency
- Works everywhere
- No additional tools needed
- Lightweight

### Cons
- Requires manual file synchronization
- No automatic configuration
- Repetitive typing of server details
- Need to manage multiple terminal sessions
- File sync easy to forget or get wrong

### biwa Advantage
biwa automates the synchronization and command execution workflow. It handles the SSH connection, file syncing, and command execution in one step, while still giving you the transparency and control of raw SSH.

---

## Feature Comparison Table

| Feature | biwa | VSCode Remote SSH | SSH FS | Raw SSH |
|---------|------|-------------------|--------|---------|
| Editor agnostic | ✅ | ❌ | ✅ | ✅ |
| Low latency editing | ✅ | ❌ | ❌ | ✅* |
| Auto file sync | ✅ | ✅ | ✅ | ❌ |
| Lightweight | ✅ | ❌ | ✅ | ✅ |
| Remote execution | ✅ | ✅ | ❌** | ✅ |
| Easy setup | ✅ | ✅ | ~~ | ✅ |
| Integrated workflow | ✅ | ✅ | ~~ | ❌ |

\* After manually copying files  
\*\* Requires separate SSH connection

---

## When to Use What

- **Use biwa** when you want fast local editing with automated remote execution
- **Use VSCode Remote SSH** when you need full IDE features on remote system
- **Use SSH FS** when you need persistent file access across multiple tools
- **Use raw SSH** when you need maximum control or are doing quick one-off tasks

## The biwa Philosophy

biwa is designed for the common case: you want to **edit code locally** with your preferred tools and setup, but **run it on remote CSE infrastructure** for testing, compilation, or submission. It optimizes for this workflow without forcing you into a specific editor or requiring heavy remote resources.

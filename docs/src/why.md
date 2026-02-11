# Why biwa?

**biwa** is a modern CLI tool designed to bridge the gap between local comfort and remote necessity. It allows you to **develop locally** with your preferred tools while seamlessly running code on UNSW CSE infrastructure.

## Core Philosophy: Develop Locally

The core philosophy of biwa is simple: **You should develop on your own machine.**

CSE servers are shared resources. They are not designed to host VS Code servers for hundreds of students, nor do they have the disk space for modern `node_modules` or Rust build artifacts.

biwa enables a **Local-First Workflow**:
1. **Edit Locally**: Use VS Code, Neovim, IntelliJ, or any editor you love with zero latency.
2. **Build Locally**: Run fast feedback loops on your own hardware.
3. **Execute Remotely**: When you need to run `autotest`, `give`, or use specific CSE compilers, biwa handles it instantly.

## The Problem with Remote Development

When working on UNSW CSE coursework, you often face a dilemma: work locally comfortably but struggle with submission/testing, or work remotely on CSE servers but deal with latency and restrictions.

### Challenges of CSE Servers
- **Latency & Disconnects**: SSH connections can be unstable, leading to laggy typing or dropped connections.
- **Disk Quotas**: CSE servers have strict disk limits. Installing modern tools or even running `npm install` can quickly exhaust your quota.
- **Resource Limits**: Heavy processes are killed, and persistent background tasks are discouraged.
- **Tooling Limitations**: You can't easily install your favorite shell, editor plugins, or system dependencies.

## Why Not Alternatives?

### Raw SSH
*The traditional approach: `ssh z1234567@cse.unsw.edu.au`*

- **Manual Sync**: You must manually copy files back and forth (`scp`/`rsync`).
- **Context Switching**: Constant switching between local editor and remote terminal breaks flow.
- **Repetitive**: Typing server details and passwords/keys repeatedly.

### VS Code Remote SSH
*The popular extension*

- **Resource Heavy**: Runs a full node.js server on the host. This consumes significant RAM and CPU.
- **Unstable on CSE**: CSE servers often kill these resource-heavy "zombie" processes. This forcibly disconnects your session, requiring a window reload and often losing terminal state.
- **Banned/Discouraged**: Due to the resource usage, its use is often discouraged or even restricted on shared servers.

### SSH FS / SFTP Extensions
*The filesystem mount approach*

- **Slow Performance**: Every file save or read requires a network round-trip.
- **Disk Usage**: Often requires caching files or installing dependencies remotely (e.g., `node_modules`), which eats into your disk quota.
- **Network Dependency**: If your internet blips, your editor hangs.

### VNC / VLab
*The graphical desktop approach*

- **High Latency**: transmitting a full desktop UI over the internet is bandwidth-intensive and feels sluggish.
- **Overkill**: You usually just need a terminal and an editor, not a full Linux desktop environment.

## The biwa Solution

biwa handles the complexity so you don't have to.

### Seamless Remote Execution
When you need to run a CSE-specific command (like `autotest`, `give`, or a specific compiler version), biwa handles it instantly:
- **Smart Sync**: Uses `rsync` to synchronize *only changed files* instantly.
- **Auto-Cleanup**: Manages remote directories automatically. If you haven't touched a project in a while, biwa cleans it up to save your remote disk quota.
- **Transient**: No heavy background processes left running on the server.

### Better than `cserun`
biwa is the spiritual successor to `cserun`, modernized for today's workflow. We are grateful to the authors of `cserun` for pioneering this approach.

Comparison:
- **Rust-Powered**: Fast, reliable, and binary-distributed.
- **Active Maintenance**: Built to be maintained and improved.
- **Smart Environment**: No need to manually set environments for every call.
- **Symlink Support**: Seamlessly handles CSE symlinks (like `1511` or specific standard libraries).

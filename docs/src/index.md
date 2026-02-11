---
layout: home

hero:
  name: "biwa"
  text: "Execute commands on UNSW CSE servers"
  tagline: "A modern CLI tool for seamless remote development on CSE infrastructure"
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/risu729/biwa

features:
  - title: Fast & Efficient
    details: Built in Rust for blazing-fast performance and minimal resource usage
  - title: Simple Configuration
    details: Easy setup with intuitive configuration files and automatic initialization
  - title: Developer Friendly
    details: Designed for modern workflows with mise integration and comprehensive tooling
---

## About

**biwa** is a CLI tool for executing commands on UNSW CSE servers from your local machine. It's inspired by [cserun](https://github.com/Bogay/cserun), a tool created by community members to simplify remote development on CSE infrastructure.

### Evolution from cserun

While biwa builds on the foundation laid by cserun, it introduces several improvements:

- **Rust implementation**: Complete rewrite in Rust for better performance and reliability
- **Modern tooling**: Integration with contemporary development tools and workflows
- **Enhanced features**: Additional capabilities planned for improved developer experience
- **Active maintenance**: Ongoing development and community support

We're grateful to the cserun project and its contributors for pioneering this approach to CSE remote development.

## Why biwa?

Working with remote servers often involves juggling multiple tools and configurations. biwa simplifies this by providing:

- **Unified interface**: Single command to execute remotely
- **Transparent operation**: Feels like running commands locally
- **Configuration management**: Easy server and project setup
- **Integration ready**: Works seamlessly with your existing tools

## Quick Example

```bash
# Initialize configuration
biwa init

# Run commands on remote CSE server
biwa run cargo test
biwa run npm start
```

Learn more in the [Getting Started](/getting-started) guide.

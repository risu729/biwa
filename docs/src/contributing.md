# Contributing

Thank you for your interest in contributing to biwa! This project is a volunteer effort and is **not associated with UNSW CSE**.

## Getting Started

The only requirements to contribute are **mise**:

1. Install **mise** (if you haven't already): [mise.jdx.dev](https://mise.jdx.dev)
2. Install dependencies:
   ```bash
   mise install
   ```

That's it! The environment is automatically managed by mise and `mise.toml`.

## Guidelines

- **Contributions Welcome**: We welcome all contributions, including bug fixes, features, and documentation improvements. Even small **typo fixes** are highly appreciated!
- **AI Contributions**: We allow AI-generated code, but **you must understand the changes**. Submitting blindly generated code that you cannot explain or debug is discouraged. Please verify that your AI-generated code works as intended.
- **Please be considerate**: Maintainers are volunteers. We might not have time to check every PR immediately, or fix every issue.
- **No guarantees**: We may not merge your PR if it doesn't align with the project's goals or quality standards.
- **Communication**: Opening an issue to discuss major changes before submitting a PR is recommended.

## Development Workflow

1. Fork the repository.
2. Create a feature branch.
3. Make your changes.
4. Run tests and linters:
   ```bash
   mise run test
   mise run check
   ```
5. Submit a Pull Request.

## Documentation

To work on documentation:

```bash
mise run docs:dev
```

This starts a local development server where you can preview your changes.

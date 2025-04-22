# Contributing to Spares

Thank you for your interest in contributing to Spares! We welcome all forms of contributions, including:

- Bug reports
- Feature requests
- Documentation improvements
- Code contributions
- Testing and feedback

## Getting Started

1. Fork the repository
2. Clone your fork:
   ```sh
   git clone https://github.com/your-username/spares.git
   cd spares
   ```
3. Create a new branch for your changes:
   ```sh
   git checkout -b your-branch-name
   ```

## Development Environment

### Prerequisites
- Rust 1.85 or later
- Cargo (comes with Rust)

### Building
```sh
cargo build
```

### Testing
```sh
# Run all tests
cargo test --features testing

# Run specific test
cargo test test_name

# Run tests with logging
RUST_LOG=debug cargo test
```

### Code Coverage
```sh
# Generate and open coverage report
cargo llvm-cov --open
```

### Profiling
```sh
# Generate flamegraph
RUST_LOG="info" sudo -E cargo flamegraph --bin spares_server
open flamegraph.svg
```

## Code Style

We follow the Rust community's coding standards. Please run the following before submitting a PR:

```sh
# Format code
cargo fmt

# Check for common issues
cargo clippy
```

## Documentation

- Keep documentation up to date with your changes
- Add comments for complex logic
- Update the relevant documentation files
- Follow the existing documentation style

## Submitting Changes

1. Ensure all tests pass
2. Format your code
3. Update documentation if needed
4. Commit your changes with a descriptive message
5. Push to your fork
6. Create a Pull Request

### Pull Request Guidelines

- Provide a clear description of the changes
- Reference any related issues
- Include tests for new features or bug fixes
- Update documentation as needed
- Keep PRs focused and manageable in size

## Issue Reporting

When reporting issues:
- Use the issue template
- Provide detailed steps to reproduce
- Include relevant logs or error messages
- Specify your environment (OS, Rust version, etc.)

## Feature Requests

For feature requests:
- Explain the problem you're trying to solve
- Describe your proposed solution
- Provide use cases or examples
- Consider potential impacts on existing features

## Code of Conduct

Please review and follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Questions?

Feel free to:
- Open an issue for questions
- Join our community discussions
- Contact the maintainers directly

Thank you for contributing to Spares!

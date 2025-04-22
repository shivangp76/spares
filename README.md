# Spares

**Spa**ced **Re**petition **S**ystem - A file-based note system built for programmers.

Spares was built to address the numerous drawbacks of Anki. See [this](https://github.com/shivangp76/spares/blob/main/docs/src/comparison.md) for a more detailed comparison with Anki.

> [!WARNING]
> Spares is in alpha. Expect frequent breaking changes.

## Features

### Core Features
- **File-based System**: All notes are stored as individual plain text files, enabling easy searching with standard tools like `ripgrep`
- **Multiple Markup Languages**: Support for Markdown, LaTeX, and Typst notes
- **Advanced Cloze System**:
  - Customizable cloze delimiters (not limited to double curly brackets)
  - Automatic cloze numbering
- **Flexible Architecture**:
  - All cards are fundamentally cloze-based
  - Decks implemented as tags
  - Swappable spaced repetition algorithms

### Advanced Features
- **Image Occlusion**:
  - Integrated as a cloze type (not a separate note type)
  - Support for interweaving image occlusion with text clozes
  - Hint display over image clozes
- **Cross-note Referencing**:
  - Keyword-based linking system
  - Support for partial references
  - Multiple source citations
- **Tag Hierarchy**: Tree-like structure for organizing notes
- **LaTeX Integration**:
  - File-based templates that automatically update
  - Background parallel rendering

### Interface and Tools
- **CLI Interface**: Full-featured command-line interface (GUI optional)
- **Anki Migration**: Easy import from Anki decks
- **Customizable**: Create your own parsers and workflows

## Quick Start

See our [Getting Started Guide](https://github.com/shivangp76/spares/blob/main/docs/src/getting_started.md).

## Documentation

- [Getting Started](https://github.com/shivangp76/spares/blob/main/docs/src/getting_started.md)
- [Concepts](https://github.com/shivangp76/spares/blob/main/docs/src/concepts.md)
- [Searching](https://github.com/shivangp76/spares/blob/main/docs/src/searching.md)
- [Workflows](https://github.com/shivangp76/spares/blob/main/docs/src/workflows.md)
- [Comparison with Anki](https://github.com/shivangp76/spares/blob/main/docs/src/comparison.md)
- [Roadmap](https://github.com/shivangp76/spares/blob/main/docs/src/roadmap.md)

## Contributing

We welcome contributions! Please see our [Contributing Guide](https://github.com/shivangp76/spares/blob/main/docs/src/contributing.md) for details.

## License

This project is licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

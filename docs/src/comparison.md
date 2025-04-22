# Comparison

## Anki Issues

The following are issues in Anki that this project aims to address:

### File Management and Search
- Notes should be stored as individual files on the system for easy searching using standard tools like `ripgrep`
- LaTeX rendering should be parallelized and performed in the background

### Format and Syntax Support
- Support for multiple markup languages (currently only HTML is supported)
- Customizable cloze delimiters (currently limited to double curly brackets. Ex: `{{c1::text}}`)
  - Current limitation causes issues with LaTeX math expressions like `2^{2^{3}}`
  - See [this Reddit post](https://www.reddit.com/r/Anki/comments/a6grv9/anki_latex_cloze_1st_cloze_works_fine_2nd_cloze/)
- Automatic cloze numbering (currently requires manual numbering)
  - See `orders` in <https://docs.rs/spares/latest/spares/spares/parsers/struct.ClozeGroupingSettings.html>

### Architecture and Design
- Improved abstractions:
  - All cards are fundamentally cloze-based (front/back cards are a special case)
  - Decks should be implemented as tags
- LaTeX template management:
  - Templates should be file-based and automatically update. This allows easy integration of LaTeX templates with your other documents.

### Image Occlusion
- Image occlusion should be a cloze type, not a separate note type. In other words, there should be support for interweaving image occlusion with text clozes.
- Image occlusion cards should be able to be interweaved with text clozes. Additionally, clozes from the image should be able to be grouped with text clozes. In order words, image occlusion is not a note type, but a cloze type.
- Enable displaying hints over image occlusion clozes

### Interface and Features
- No built-in CLI (GUI should be optional)
- Tag hierarchy support for tree-like structures
- Keyword system for efficient note querying and linking
- Flexible spaced repetition algorithm implementation
- Cross-note referencing:
  - Support citing across keywords
  - No forced title field requirement
  - Allow citing partial references (e.g., "Ransford")
  - Support multiple source citations for the same content

### Development Issues
- Poor database structure design
  - See <https://natemeyvis.com/on-ankis-database/> for technical details

## Current Limitations of spares

- No mobile app or web interface
- No card-level flagging system (tags are note-level only)
- No support for typed cloze answers
- No undo functionality
- No cloud backup system

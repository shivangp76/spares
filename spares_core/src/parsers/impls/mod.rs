//! The parsers spares ships with: [`markdown`], [`typst`] and [`latex`] (`latex-note`).
//!
//! Parsers allow notes to be created in different markup languages. They are meant to be modified
//! by the user. A markup language can have multiple parsers. For example, you may have a parser
//! called `LatexMath` for math notes and `LatexChem` for chemistry notes. This would allow you to have
//! different preambles since chemistry LaTeX packages will not be needed for math notes and vice
//! versa.
//!
//! # Creating a parser
//!
//! The "markdown" parser serves as a guide for a basic parser, while the "latex-note" parser is
//! more advanced. The "typst" parser is a middle ground. If you are creating your own parser, it is
//! probably best to copy one of these as your starting template and implement
//! [`Parseable`](crate::parsers::Parseable) for it.
//!
//! Creating or modifying a parser requires recompiling spares. A parser is then added to the
//! database with `spares parser add --name <name>` before notes can use it.

pub mod latex;
pub mod markdown;
pub mod typst;

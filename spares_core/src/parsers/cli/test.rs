use indoc::indoc;
use pretty_assertions::assert_eq;

use crate::parsers::Parseable;
use crate::parsers::cli::compute_surrounding_text;
use crate::parsers::cli::parse_cli_data;
use crate::parsers::impls::markdown::MarkdownParser;

#[test]
fn test_parse_cli_block_basic() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Some prompt text.
        <!--- spares: cli start --->
        <!--- exec = "pytest tests/" --->
        <!--- spares: cli end --->
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    assert_eq!(blocks.len(), 1);
    let (cli_data, _range) = &blocks[0];
    assert_eq!(cli_data.exec, "pytest tests/");
}

#[test]
fn test_parse_cli_block_escaping() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "echo \"hi\" && echo {\"score\": 0.5}" --->
        <!--- spares: cli end --->
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    assert_eq!(blocks.len(), 1);
    let (cli_data, _range) = &blocks[0];
    assert_eq!(cli_data.exec, r#"echo "hi" && echo {"score": 0.5}"#);
}

#[test]
fn test_parse_cli_block_missing_exec_errors() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- some_unknown_key = "value" --->
        <!--- spares: cli end --->
    "#};
    let result = parse_cli_data(&parser, data);
    assert!(result.is_err(), "expected error for missing exec");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("unknown field `some_unknown_key`") || msg.contains("missing field `exec`"),
        "unexpected error: {msg}"
    );
}

#[test]
fn test_construct_cli_block_round_trips_via_markdown() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Prompt.
        <!--- spares: cli start --->
        <!--- exec = "pytest" --->
        <!--- spares: cli end --->
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    let (cli_data, _range) = &blocks[0];
    let reconstructed = parser.construct_cli_block(cli_data);
    let reparsed = parse_cli_data(&parser, &reconstructed).unwrap();
    assert_eq!(reparsed.len(), 1);
    assert_eq!(reparsed[0].0, *cli_data);
}

#[test]
fn test_get_cli_blocks_trait_and_helper_produce_same_results() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "true" --->
        <!--- spares: cli end --->
    "#};
    let via_trait = parser.get_cli_blocks(data).unwrap();
    let via_helper = crate::parsers::cli::get_cli_blocks(&parser, data).unwrap();
    assert_eq!(via_trait, via_helper);
}

#[test]
fn test_compute_surrounding_text_single_block_middle() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Before text.
        <!--- spares: cli start --->
        <!--- exec = "true" --->
        <!--- spares: cli end --->
        After text.
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    assert_eq!(blocks.len(), 1);
    let surrounding = compute_surrounding_text(data, &blocks);
    assert!(
        surrounding.contains("Before text."),
        "surrounding={surrounding:?}"
    );
    assert!(
        surrounding.contains("After text."),
        "surrounding={surrounding:?}"
    );
    assert!(!surrounding.contains("exec"), "surrounding={surrounding:?}");
    assert!(
        !surrounding.contains("spares: cli"),
        "surrounding={surrounding:?}"
    );
}

#[test]
fn test_compute_surrounding_text_no_blocks_is_whole_text() {
    let data = "just text";
    let surrounding = compute_surrounding_text(data, &[]);
    assert_eq!(surrounding, "just text");
}

#[test]
fn test_parse_cli_block_unterminated_errors() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Some prompt text.
        <!--- spares: cli start --->
        <!--- exec = "pytest" --->
        <!-- missing end marker -->
    "#};
    let result = parse_cli_data(&parser, data);
    assert!(result.is_err(), "expected error for unterminated CLI block");
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("Unterminated"), "unexpected error: {msg}");
}

#[test]
fn test_parse_cli_block_first_unterminated_second_terminated_errors() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "a" --->
        <!-- first block is missing end marker -->
        <!--- spares: cli start --->
        <!--- exec = "b" --->
        <!--- spares: cli end --->
    "#};
    let result = parse_cli_data(&parser, data);
    assert!(
        result.is_err(),
        "expected error when first of two CLI blocks is unterminated"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("contains another"), "unexpected error: {msg}");
}

#[test]
fn test_parse_cli_data_multi_block() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Start.
        <!--- spares: cli start --->
        <!--- exec = "a" --->
        <!--- spares: cli end --->
        Middle.
        <!--- spares: cli start --->
        <!--- exec = "b" --->
        <!--- spares: cli end --->
        End.
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].0.exec, "a");
    assert_eq!(blocks[1].0.exec, "b");
    assert!(blocks[0].1.start < blocks[1].1.start);
}

#[test]
fn test_compute_surrounding_text_multi_block() {
    let parser = MarkdownParser::new();
    let data = indoc! {r#"
        Start.
        <!--- spares: cli start --->
        <!--- exec = "a" --->
        <!--- spares: cli end --->
        Middle.
        <!--- spares: cli start --->
        <!--- exec = "b" --->
        <!--- spares: cli end --->
        End.
    "#};
    let blocks = parse_cli_data(&parser, data).unwrap();
    assert_eq!(blocks.len(), 2);
    let surrounding = compute_surrounding_text(data, &blocks);
    assert!(surrounding.contains("Start."));
    assert!(surrounding.contains("Middle."));
    assert!(surrounding.contains("End."));
    assert!(!surrounding.contains("exec"));
}

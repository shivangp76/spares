use pretty_assertions::assert_eq;

use crate::parsers::BackReveal;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ClozeGrouping;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::FrontConceal;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::get_cards;
use crate::parsers::get_cloze_context_for_card_order;
use crate::parsers::impls::markdown::MarkdownParser;
use crate::parsers::impls::typst::TypstParser;

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_basic_1_markdown() {
    let data = r"a {{ b }} c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a ".to_string()),
                NotePart::ClozeStart("{{[o:1]".to_string()),
                NotePart::ClozeData(
                    " b ".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData(" c".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: None,
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a ".to_string()),
                NotePart::ClozeStart("{{".to_string()),
                NotePart::ClozeData(
                    " b ".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData(" c".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_add_order_1() {
    // It is okay to specify the order when calling with `add_order = true`.
    let data = r"a{{[o:1]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            previous_order: Some(1),
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[o:1]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("c".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_add_order_2() {
    // Since `add_order = true`, the incorrect order will be corrected.
    let data = r"a{{[o:2]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
}

#[test]
fn test_get_cards_add_order_3() {
    // Since `add_order = true`, the incorrect (missing) order will be corrected.
    let data = r"a{{[o:1;r:]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
}

#[test]
fn test_get_cards_order() {
    // The order is not checked when `add_order` is `false`.
    let data = r"a{{[o:2]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(2),
            previous_order: Some(2),
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[o:2]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("c".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_hint() {
    let data = r"{{[h:this is a hint]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: None,
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::ClozeStart("{{[h:this is a hint]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer {
                        hint: Some("this is a hint".to_string()),
                    },
                ),
                NotePart::ClozeEnd("}}".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_hidden_1() {
    let data = r"a{{[g:1;hide:]b}}{{[g:1]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: None,
            previous_order: None,
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;hide:]".to_string()),
                NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::ClozeStart("{{[g:1]".to_string()),
                NotePart::ClozeData(
                    "c".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_hidden_2() {
    let data = r"a{{[g:1;hide:]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_err());
}

#[test]
fn test_get_cards_hidden_3() {
    let data = r"{{[g:1;hide:; g:3]a}}{{[g:1,2; g:3;hide:]b}}{{[g:2]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        // Cards:
        let expected = vec![
            // _a_ _ c
            CardData {
                order: None,
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:1;hide:; g:3]".to_string()),
                    NotePart::ClozeData("a".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:3;hide:; g:2]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{[g:2]c}}".to_string()),
                ],
            },
            // _ _b_ c
            CardData {
                order: None,
                previous_order: None,
                grouping: ClozeGrouping::Custom("3".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:1;hide:; g:3]".to_string()),
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:3;hide:; g:2]".to_string()),
                    NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{[g:2]c}}".to_string()),
                ],
            },
            // a _ _
            CardData {
                order: None,
                previous_order: None,
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("{{[g:1;hide:; g:3]a}}".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:3;hide:; g:2]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:2]".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_empty_cloze() {
    let data = r"a{{}}b";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_err());
    assert_eq!(
        cards_res.unwrap_err().to_string(),
        "Empty clozes are not allowed.".to_string()
    );
}

#[test]
fn test_get_cards_no_clozes() {
    let data = "a\nb";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        assert!(cards.is_empty());
    }
}

#[test]
fn test_get_cloze_context_for_card_order_markdown_basic() {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());

    // Single card — should include preceding text in context
    let data = "intro {{ hidden }} outro";
    let ctx = get_cloze_context_for_card_order(parser.as_ref(), data, 1)
        .unwrap()
        .unwrap();
    assert!(
        ctx.contains("intro "),
        "context should include preceding text"
    );
    assert!(ctx.contains("{{"), "context should include cloze start");
    assert!(
        ctx.contains("hidden"),
        "context should include cloze content"
    );

    // Order out of range
    assert!(
        get_cloze_context_for_card_order(parser.as_ref(), data, 2)
            .unwrap()
            .is_none()
    );

    // Order 0 is always None
    assert!(
        get_cloze_context_for_card_order(parser.as_ref(), data, 0)
            .unwrap()
            .is_none()
    );

    // No clozes
    let data_no_cloze = "just plain text";
    assert!(
        get_cloze_context_for_card_order(parser.as_ref(), data_no_cloze, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_get_cloze_context_for_card_order_two_cards() {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());

    // Two independent clozes → two cards; each context should contain its own cloze
    let data = "prefix1 {{ card1 }} middle {{ card2 }} suffix";
    let ctx1 = get_cloze_context_for_card_order(parser.as_ref(), data, 1)
        .unwrap()
        .unwrap();
    let ctx2 = get_cloze_context_for_card_order(parser.as_ref(), data, 2)
        .unwrap()
        .unwrap();

    assert!(ctx1.contains("card1"), "ctx1 should include card1 content");
    assert!(ctx2.contains("card2"), "ctx2 should include card2 content");
    // ctx1 should NOT extend past its own cloze end
    assert!(
        !ctx1.contains("card2"),
        "ctx1 should not include card2 content"
    );
}

#[test]
fn test_get_cloze_context_for_card_order_typst_proof() {
    let parser: Box<dyn Parseable> = Box::new(TypstParser::new());

    // Simulate the user's pattern: a cloze inside a #proof block and one outside
    let data = "#proof[\n  #cl[theorem][g:1]\n]\n#cl[outside][g:2]";
    let ctx1 = get_cloze_context_for_card_order(parser.as_ref(), data, 1)
        .unwrap()
        .unwrap();
    let ctx2 = get_cloze_context_for_card_order(parser.as_ref(), data, 2)
        .unwrap()
        .unwrap();

    // The context for card1 should include "#proof[" because it appears just before the cloze
    assert!(
        ctx1.contains("#proof["),
        "ctx1 should include surrounding #proof block: {ctx1}"
    );
    // The context for card2 should NOT start with "#proof[" since it's 500+ chars away
    // (in this short example they're close, so just check card2's content is right)
    assert!(
        ctx2.contains("#cl[outside]"),
        "ctx2 should include outside cloze: {ctx2}"
    );
}

#[test]
fn test_get_cards_cli_block_produces_single_card() {
    use indoc::indoc;
    let data = indoc! {r#"
        Run the test suite and recall score.
        <!--- spares: cli start --->
        <!--- exec = "pytest tests/" --->
        <!--- spares: cli end --->
    "#};
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards = get_cards(parser.as_ref(), None, data, true, MOVE_FILES).unwrap();
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(card.back_type, BackType::Cli);
    assert_eq!(card.order, Some(1));
    assert!(card.data.iter().any(|p| matches!(p, NotePart::Cli { .. })));
    assert!(
        card.data
            .iter()
            .any(|p| matches!(p, NotePart::SurroundingData(_)))
    );
}

#[test]
fn test_get_cards_cli_block_mix_with_cloze_errors() {
    use indoc::indoc;
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "pytest" --->
        <!--- spares: cli end --->
        {{ should not be allowed }}
    "#};
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let result = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(result.is_err(), "expected mutual-exclusion error");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("cannot contain both a CLI block"),
        "unexpected error: {msg}"
    );
}

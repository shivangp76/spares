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

/// Every CLI block in a note gets its own card, each with its own uid.
#[test]
fn test_get_cards_cli_multi_block_produces_one_card_each() {
    use indoc::indoc;
    let data = indoc! {r#"
        First task.
        <!--- spares: cli start --->
        <!--- exec = "task one" --->
        <!--- spares: cli end --->
        Second task.
        <!--- spares: cli start --->
        <!--- exec = "task two" --->
        <!--- spares: cli end --->
    "#};
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards = get_cards(parser.as_ref(), None, data, true, MOVE_FILES).unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].order, Some(1));
    assert_eq!(cards[1].order, Some(2));
    let uids = cards
        .iter()
        .map(|c| c.cloze_uid.expect("every block should be given a uid"))
        .collect::<Vec<_>>();
    assert_ne!(uids[0], uids[1], "each block gets its own uid");
}

/// Note text must survive card assembly untouched apart from the minted `id` lines. Card parts are
/// what the note is rebuilt from, so a card carrying only its own block would drop the others.
#[test]
fn test_add_order_to_note_data_cli_preserves_all_blocks_and_positions() {
    use crate::parsers::add_order_to_note_data;
    use crate::parsers::cli::parse_cli_data;
    use indoc::indoc;

    let data = indoc! {r#"
        Intro text.
        <!--- spares: cli start --->
        <!--- exec = "task one" --->
        <!--- spares: cli end --->
        Text between the blocks.
        <!--- spares: cli start --->
        <!--- exec = "task two" --->
        <!--- spares: cli end --->
        Trailing text.
    "#};
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let (new_data, cards) = add_order_to_note_data(parser.as_ref(), data, None).unwrap();
    assert_eq!(cards.len(), 2);

    // Both blocks survive, in order, with their surrounding text still in place.
    let blocks = parse_cli_data(parser.as_ref(), &new_data).unwrap();
    assert_eq!(blocks.len(), 2, "both blocks must survive: {new_data}");
    assert_eq!(blocks[0].0.exec, "task one");
    assert_eq!(blocks[1].0.exec, "task two");
    for text in ["Intro text.", "Text between the blocks.", "Trailing text."] {
        assert!(new_data.contains(text), "lost `{text}`: {new_data}");
    }
    let between = new_data.find("Text between the blocks.").unwrap();
    assert!(
        blocks[0].1.end <= between && between <= blocks[1].1.start,
        "text between the blocks must stay between them: {new_data}"
    );

    // The only difference from the input is the minted `id` lines.
    let stripped = new_data
        .lines()
        .filter(|line| !line.contains("id = "))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(stripped.trim_end(), data.trim_end());

    // Each card's uid matches its own block, so cards stay bound to their own command.
    assert_eq!(cards[0].cloze_uid, blocks[0].0.id);
    assert_eq!(cards[1].cloze_uid, blocks[1].0.id);
}

/// A parse that only reads stored data must not invent uids: minting on both sides of an update
/// would give the old and new cards different uids and match nothing.
#[test]
fn test_get_cards_cli_without_add_order_does_not_mint() {
    use indoc::indoc;
    let data = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "pytest" --->
        <!--- spares: cli end --->
    "#};
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards = get_cards(parser.as_ref(), None, data, false, MOVE_FILES).unwrap();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].cloze_uid.is_none());

    // An id already in the text is read back on both paths.
    let with_id = indoc! {r#"
        <!--- spares: cli start --->
        <!--- exec = "pytest" --->
        <!--- id = "a1b2c3d4e5f6" --->
        <!--- spares: cli end --->
    "#};
    for add_order in [false, true] {
        let cards = get_cards(parser.as_ref(), None, with_id, add_order, MOVE_FILES).unwrap();
        assert_eq!(cards[0].cloze_uid.unwrap().to_string(), "a1b2c3d4e5f6");
    }
}

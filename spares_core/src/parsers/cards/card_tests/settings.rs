use pretty_assertions::assert_eq;

use crate::parsers::BackReveal;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ClozeGrouping;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::FrontConceal;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::ReadableCardIdentifier;
use crate::parsers::get_cards;
use crate::parsers::impls::markdown::MarkdownParser;

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_front_conceal_1() {
    let data = r"a{{b}}c{{d{{[g:1;f:all]e}}f{{[g:1]g}}h}}i";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
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
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData(
                        "c{{[o:2]d{{[g:1;o:3;f:all]e}}f{{[g:1]g}}h}}i".to_string(),
                    ),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: None,
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d{{[g:1;o:3;f:all]e}}f{{[g:1]g}}h".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("i".to_string()),
                ],
            },
            CardData {
                order: Some(3),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::AllGroupings,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:3;f:all]".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("f".to_string()),
                    NotePart::ClozeStart("{{[g:1]".to_string()),
                    NotePart::ClozeData(
                        "g".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("h}}i".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_front_conceal_2() {
    // Test `front_conceal` on a cloze where the other clozes are nested. In this example, order the clozes as 1, 2, 3 by their starting position. Then, for cloze 1 where we have `f:all`, we only need to get cloze 2 to be hidden. This is because cloze 3 is nested inside cloze 2 so it will automatically be hidden.
    let data = r"a{{[f:all]b}}c{{d{{e}}f}}i";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::AllGroupings,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1;f:all]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d{{[o:3]e}}f".to_string(),
                        ClozeHiddenReplacement::NotToAnswer,
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("i".to_string()),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: None,
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1;f:all]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d{{[o:3]e}}f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("i".to_string()),
                ],
            },
            CardData {
                order: Some(3),
                previous_order: None,
                grouping: ClozeGrouping::Auto(3),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1;f:all]b}}c{{[o:2]d".to_string()),
                    NotePart::ClozeStart("{{[o:3]".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("f}}i".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_back_reveal_1() {
    let data = r"a{{[b:a]b}}c";
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
            back_reveal: BackReveal::OnlyAnswered,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            cloze_uid: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[o:1;b:a]".to_string()),
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
fn test_get_cards_back_reveal_2() {
    let data = r"a{{b}}c{{d{{[g:1;f:all;b:a]e}}f{{[g:1]g}}h}}i";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
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
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData(
                        "c{{[o:2]d{{[g:1;o:3;f:all;b:a]e}}f{{[g:1]g}}h}}i".to_string(),
                    ),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: None,
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d{{[g:1;o:3;f:all;b:a]e}}f{{[g:1]g}}h".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("i".to_string()),
                ],
            },
            CardData {
                order: Some(3),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::AllGroupings,
                back_reveal: BackReveal::OnlyAnswered,
                back_emphasis: false,
                back_type: BackType::CardFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:3;f:all;b:a]".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("f".to_string()),
                    NotePart::ClozeStart("{{[g:1]".to_string()),
                    NotePart::ClozeData(
                        "g".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("h}}i".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_back_reveal_err() {
    // If there is more than 1 grouping, then `FrontConceal::OnlyGrouping` and `BackReveal::OnlyAnswered` cannot both be set. This would mean the other groupings are visible on the front, but hidden on the back, even though they are not tested. Either change `front_conceal`, change `back_reveal`, or remove a grouping.
    let data = r"a{{b}}c{{[b:a]d}}e";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_err());
}

#[test]
fn test_get_cards_suspended_only_deserialized() {
    // Tests that is suspended is only deserialized, not serialized
    let data = r"a{{[s:]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: Some(true),
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
fn test_get_cards_suspended_false() {
    // Tests that is suspended can explicitly be no with `s:n`
    let data = r"a{{[s:n]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: Some(false),
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
fn test_get_cards_previous_order_reorder() {
    // Simulate a note that previously had 2 cards (old orders 1 and 2) whose positions
    // were swapped. The submitted text references old order 2 first and old order 1 second.
    // After `add_order = true`, orders are renumbered sequentially while `previous_order`
    // preserves the old references so `match_cards` can reconcile them with the database.
    let data = r"a{{[o:2]b}}c{{[o:1]d}}e";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
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
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d}}e".to_string()),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: Some(1),
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_previous_order_new_card() {
    // Simulate a note that previously had 2 cards (old orders 1 and 2) with a brand-new
    // card (no embedded order) inserted between them. After `add_order = true`, the three
    // cards get sequential orders 1, 2, 3. The new card has `previous_order = None`; the
    // card that was at old order 2 now has `previous_order = Some(2)` at new position 3.
    let data = r"a{{[o:1]b}}c{{d}}e{{[o:2]f}}g";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
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
                    NotePart::SurroundingData("c{{[o:2]d}}e{{[o:3]f}}g".to_string()),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: None,
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e{{[o:3]f}}g".to_string()),
                ],
            },
            CardData {
                order: Some(3),
                previous_order: Some(2),
                grouping: ClozeGrouping::Auto(3),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c{{[o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[o:3]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_previous_order_with_reverse() {
    // Simulate a note with a regular card (old order 2) followed by a reverse cloze that
    // previously occupied old orders 3 (forward) and 4 (reverse). After `add_order = true`,
    // all three cards are renumbered 1, 2, 3 while `previous_order` tracks each direction's
    // old order independently, which lets `match_cards` correctly update the database rows.
    let data = r"a{{[o:2]b}}c{{[o:3,4;r:]d}}e";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
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
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2,3;r:]d}}e".to_string()),
                ],
            },
            CardData {
                order: Some(2),
                previous_order: Some(3),
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2,3;r:]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
                ],
            },
            CardData {
                order: Some(3),
                previous_order: Some(4),
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                cloze_uid: None,
                data: vec![
                    NotePart::ClozeData(
                        "a{{[o:1]b}}c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeStart("{{[o:2,3;r:]".to_string()),
                    NotePart::SurroundingData("d".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_inherit_basic() {
    // `inh:NOTE_ID/ORDER` should be parsed into `CardData.inherit` and stripped from the output.
    let data = r"{{[inh:123/1]b}}";
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
            inherit: Some(ReadableCardIdentifier {
                note_id: 123,
                order: 1,
            }),
            cloze_uid: None,
            data: vec![
                // `inh:` is not serialized, so ClozeStart has no settings.
                NotePart::ClozeStart("{{".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_inherit_with_order() {
    // `inh:` can be combined with other settings; only `inh:` is stripped from the output.
    let data = r"{{[o:1;inh:456/2]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
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
            inherit: Some(ReadableCardIdentifier {
                note_id: 456,
                order: 2,
            }),
            cloze_uid: None,
            data: vec![
                NotePart::ClozeStart("{{[o:1]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_inherit_negative_note_id() {
    // Negative note IDs are valid i64 values and should be accepted.
    let data = r"{{[inh:-99/3]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        assert_eq!(
            cards[0].inherit,
            Some(ReadableCardIdentifier {
                note_id: -99,
                order: 3,
            })
        );
    }
}

#[test]
fn test_get_cards_inherit_invalid_note_id() {
    // A non-integer note ID should produce an error.
    let data = r"{{[inh:abc/1]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_err());
    let err = cards_res.unwrap_err().to_string();
    assert!(
        err.contains("inh:"),
        "expected error mentioning `inh:`, got: {err}"
    );
}

#[test]
fn test_get_cards_inherit_invalid_order() {
    // A non-integer order should produce an error.
    let data = r"{{[inh:123/abc]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_err());
    let err = cards_res.unwrap_err().to_string();
    assert!(
        err.contains("inh:"),
        "expected error mentioning `inh:`, got: {err}"
    );
}

#[test]
fn test_get_cards_inherit_zero_order() {
    // Order 0 is invalid; orders must be >= 1.
    let data = r"{{[inh:123/0]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_err());
    let err = cards_res.unwrap_err().to_string();
    assert!(
        err.contains("inh:"),
        "expected error mentioning `inh:`, got: {err}"
    );
}

#[test]
fn test_get_cards_inherit_missing_order() {
    // A value without the `/ORDER` part should fail to parse.
    let data = r"{{[inh:123]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, false, MOVE_FILES);
    assert!(cards_res.is_err());
}

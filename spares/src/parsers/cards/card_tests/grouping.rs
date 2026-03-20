use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable, add_order_to_note_data, get_cards,
    impls::markdown::MarkdownParser,
};
use pretty_assertions::assert_eq;

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_grouping_1() {
    // Also tests:
    // 1. The case where an order must be added to a cloze that doesn't have any settings. This ensures that brackets are properly added around the cloze's settings.
    // 2. Card settings can be specified on any cloze within the same grouping. In this case, `reverse_only` is specified on the second cloze in grouping 1. This should still add a reverse card the card.
    // 3. Card settings boil up to the first cloze. In this case, `reverse_only` is specified on the second cloze in grouping 1. This should be boiled up the first cloze and removed from the current cloze it is specified on.
    let data = r"a{{[g:1]b}}c{{[g:1;ro:]d}}e{{f}}g{{[g:2]h}}i";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeStart("{{[g:1;o:1;ro:]".to_string()),
                    NotePart::SurroundingData("b".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeStart("{{[g:1]".to_string()),
                    NotePart::SurroundingData("d".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "e{{[o:2]f}}g{{[g:2;o:3]h}}i".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1;ro:]b}}c{{[g:1]d}}e".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g{{[g:2;o:3]h}}i".to_string()),
                ],
            },
            CardData {
            order: Some(3),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData(
                        "a{{[g:1;o:1;ro:]b}}c{{[g:1]d}}e{{[o:2]f}}g".to_string(),
                    ),
                    NotePart::ClozeStart("{{[g:2;o:3]".to_string()),
                    NotePart::ClozeData(
                        "h".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("i".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_grouping_multiple() {
    // Since grouping 2 is only defined on the last cloze, we get 3 cards:
    // 1. Grouping 1 with clozes 1 and 3
    // 2. Grouping None with cloze 2
    // 3. Grouping 2 with cloze 3
    let data = r"a{{[g:1]b}}c{{d}}e{{[g:1,2]f}}g";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:2;o:3]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e{{[g:1; g:2;o:3]f}}g".to_string()),
                ],
            },
            CardData {
            order: Some(3),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c{{[o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:2;o:3]".to_string()),
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
fn test_get_cards_grouping_all_1() {
    let data = r"a{{[g:1]b}}c{{d}}e{{[g:*]f}}g";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[g:*]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
                    NotePart::ClozeStart("{{[g:*]".to_string()),
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
fn test_get_cards_grouping_all_2() {
    // Tests:
    // 1. Grouping "*" specified first
    // 2. Grouping "*" and 1 is redundant, so it is truncated to just grouping "*"
    let data = r"{{[g:*,1]a}}{{b}}{{[g:1]c}}";
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
                data: vec![
                    NotePart::ClozeStart("{{[g:*; o:1; g:1;o:2]".to_string()),
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{[g:1]c}}".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:*; o:1; g:1;o:2]".to_string()),
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{b}}".to_string()),
                    NotePart::ClozeStart("{{[g:1]".to_string()),
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
fn test_get_cards_grouping_all_3() {
    // Grouping "*" should be _nearly_ identical to manually specifying the groupings.
    let data = r"a{{[g:1]b}}c{{[g:2]d}}e{{[g:1,2]f}}g";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res1 = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res1.is_ok());
    if let Ok(cards) = cards_res1 {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[g:2;o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[g:1,2]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
                    NotePart::ClozeStart("{{[g:1,2]".to_string()),
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

    let data = r"a{{[g:1]b}}c{{[g:2]d}}e{{[g:*]f}}g";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res2 = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res2.is_ok());
    if let Ok(cards) = cards_res2 {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[g:2;o:2]d}}e".to_string()),
                    NotePart::ClozeStart("{{[g:*]".to_string()),
                    NotePart::ClozeData(
                        "f".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("g".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
                    NotePart::ClozeStart("{{[g:*]".to_string()),
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
fn test_get_cards_grouping_all_4() {
    // Ensures that settings passed to all groupings persist. In other words, we cannot replace `g:*;hide:` with `g:*; g:1;hide:` since we also want all future groupings to have the setting `hide:`. This test highlights the need for the struct `ClozeGroupingSettings` when `g:*` is present. It is *not* sufficient to only duplicate the settings from `g:*` into settings for each grouping since this does not handle future changes.
    let data_1 = r"a{{[g:1]b}}c{{[g:*;hide:]f}}";
    // let data_1 = r"a{{[g:1]b}}c{{a}}{{[g:*;r:]f}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let (new_data, _) = add_order_to_note_data(parser.as_ref(), data_1).unwrap();
    assert!(new_data.contains("g:*;hide:;"));
    let data = r"a{{[g:1]b}}c{{d}}{{[g:*;hide:]f}}";
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[o:2]d}}".to_string()),
                    NotePart::ClozeStart("{{[g:*;hide:; g:1;hide:; hide:]".to_string()),
                    NotePart::ClozeData("f".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[g:1;o:1]b}}c".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:*;hide:; g:1;hide:; hide:]".to_string()),
                    NotePart::ClozeData("f".to_string(), ClozeHiddenReplacement::NotToAnswer),
                    NotePart::ClozeEnd("}}".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_2_cards_same_grouping_1() {
    // Creates 2 identical cards, so not allowed
    let data = r"{{[g:1,2]a}}{{[g:1,2]b}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_err());
    assert_eq!(
        cards_res.unwrap_err().to_string(),
        "Multiple cards cannot have the same clozes.".to_string()
    );
}

#[test]
fn test_get_cards_2_cards_same_grouping_2() {
    // This does NOT create 2 identical cards.
    let data = r"{{[g:1;ro:]a}}b{{[g:2]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:1;o:1;ro:]".to_string()),
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "b{{[g:2;o:2]c}}".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("{{[g:1;o:1;ro:]a}}b".to_string()),
                    NotePart::ClozeStart("{{[g:2;o:2]".to_string()),
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
fn test_get_cards_2_cards_same_grouping_3() {
    // This does NOT create 2 identical cards.
    let data = r"{{[g:1;r:]a}}{{[g:1,2]b}}{{[g:2,3]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
}

#[test]
fn test_get_cards_circular_grouping_1() {
    let data = r"{{[g:1,2]a}}{{[g:1,3]b}}{{[g:2,3]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:1;o:1; g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:3;o:3]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{[g:2,3]c}}".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::ClozeStart("{{[g:1;o:1; g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("{{[g:1; g:3;o:3]b}}".to_string()),
                    NotePart::ClozeStart("{{[g:2,3]".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                ],
            },
            CardData {
            order: Some(3),
            previous_order: None,
            grouping: ClozeGrouping::Custom("3".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("{{[g:1;o:1; g:2;o:2]a}}".to_string()),
                    NotePart::ClozeStart("{{[g:1; g:3;o:3]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeStart("{{[g:2,3]".to_string()),
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
fn test_get_cards_circular_grouping_2() {
    let data = r"{{[g:1,2;s:]a}}{{[g:1,3;r:]b}}{{[g:2,3;ro:]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
}

#[test]
fn test_get_cards_order_before_grouping() {
    let data = r"a{{[o:1;g:1]b}}c{{[g:1]d}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            previous_order: Some(1),
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("c".to_string()),
                NotePart::ClozeStart("{{[g:1]".to_string()),
                NotePart::ClozeData(
                    "d".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_grouping_multiple_times() {
    let data = r"a{{[g:1;h:Test;g:2;g:1;h:Test Override]b}}c{{[g:1]d}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                previous_order: None,
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[h:Test Override;g:1;o:1; g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer {
                            hint: Some("Test Override".to_string()),
                        },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c".to_string()),
                    NotePart::ClozeStart("{{[g:1]".to_string()),
                    NotePart::ClozeData(
                        "d".to_string(),
                        ClozeHiddenReplacement::ToAnswer {
                            hint: Some("Test Override".to_string()),
                        },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                ],
            },
            CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[h:Test Override;g:1;o:1; g:2;o:2]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer {
                            hint: Some("Test Override".to_string()),
                        },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c{{[g:1]d}}".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

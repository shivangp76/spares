use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable, get_cards,
    impls::markdown::MarkdownParser,
};
use pretty_assertions::assert_eq;

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_nested_1() {
    let data = r"a{{b{{c}}d}}e";
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
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b{{[o:2]c}}d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
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
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("d}}e".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_nested_1_reverse() {
    let data = r"a{{[r:]b{{c}}d}}e";
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
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1,2;r:]".to_string()),
                    NotePart::ClozeData(
                        "b{{[o:3]c}}d".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("e".to_string()),
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
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeStart("{{[o:1,2;r:]".to_string()),
                    NotePart::SurroundingData("b{{[o:3]c}}d".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                ],
            },
            CardData {
            order: Some(3),
            previous_order: None,
            grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
                data: vec![
                    NotePart::SurroundingData("a{{[o:1,2;r:]b".to_string()),
                    NotePart::ClozeStart("{{[o:3]".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("d}}e".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_nested_2() {
    let data = r"a{{[g:1]b{{[g:1]c}}d}}e";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_err());
    assert_eq!(
        cards_res.unwrap_err().to_string(),
        "Clozes in the same grouping can not be nested.".to_string()
    );
}

#[test]
fn test_get_cards_nested_siblings() {
    let data = r"a{{b{{c}}d{{e}}}}f";
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
                    NotePart::SurroundingData("a".to_string()),
                    NotePart::ClozeStart("{{[o:1]".to_string()),
                    NotePart::ClozeData(
                        "b{{[o:2]c}}d{{[o:3]e}}".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("f".to_string()),
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
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b".to_string()),
                    NotePart::ClozeStart("{{[o:2]".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("d{{[o:3]e}}}}f".to_string()),
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
                data: vec![
                    NotePart::SurroundingData("a{{[o:1]b{{[o:2]c}}d".to_string()),
                    NotePart::ClozeStart("{{[o:3]".to_string()),
                    NotePart::ClozeData(
                        "e".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("}}f".to_string()),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

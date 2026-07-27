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
use crate::parsers::impls::markdown::MarkdownParser;

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_reverse_1() {
    let data = r"a{{[r:]b}}c";
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
                    NotePart::ClozeStart("{{[o:1,2;r:]".to_string()),
                    NotePart::ClozeData(
                        "b".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::SurroundingData("c".to_string()),
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
                cloze_uid: None,
                data: vec![
                    NotePart::ClozeData(
                        "a".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeStart("{{[o:1,2;r:]".to_string()),
                    NotePart::SurroundingData("b".to_string()),
                    NotePart::ClozeEnd("}}".to_string()),
                    NotePart::ClozeData(
                        "c".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                ],
            },
        ];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_reverse_2() {
    let data = r"a{{[ro:]b}}c";
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
                NotePart::ClozeData(
                    "a".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeStart("{{[o:1;ro:]".to_string()),
                NotePart::SurroundingData("b".to_string()),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::ClozeData(
                    "c".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
            ],
        }];
        assert_eq!(cards, expected);
    }
}

#[test]
fn test_get_cards_reverse_3() {
    let data = r"a {{[ro:;r:] b }} c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_err());
    assert_eq!(
        cards_res.unwrap_err().to_string(),
        "`include reverse` and `reverse only` are mutually exclusive settings.".to_string()
    );
}

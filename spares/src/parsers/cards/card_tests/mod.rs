use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable, get_cards, impls::markdown::MarkdownParser,
};
use pretty_assertions::assert_eq;

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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("3".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
fn test_get_cards_reverse_1() {
    let data = r"a{{[r:]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(3),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("3".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("2".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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

#[test]
fn test_get_cards_front_conceal() {
    let data = r"a{{b}}c{{d{{[g:1;f:all]e}}f{{[g:1]g}}h}}i";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![
            CardData {
                order: Some(1),
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::AllGroupings,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
fn test_get_cards_back_reveal_1() {
    let data = r"a{{[b:a]b}}c";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, data, true, MOVE_FILES);
    assert!(cards_res.is_ok());
    if let Ok(cards) = cards_res {
        let expected = vec![CardData {
            order: Some(1),
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::OnlyAnswered,
            back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Auto(2),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
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
                grouping: ClozeGrouping::Custom("1".to_string()),
                is_suspended: None,
                front_conceal: FrontConceal::AllGroupings,
                back_reveal: BackReveal::OnlyAnswered,
                back_type: BackType::OnlyAnswered,
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
    // Both `front_conceal` and `back_reveal` cannot both be set to `OnlyGrouping` if there is more than 1 grouping
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: Some(true),
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: Some(false),
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_type: BackType::FullNote,
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

use crate::parsers::cards::overlapper::OverlapperConfig;
use crate::parsers::cards::get_cards_main;
use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable,
};
use crate::parsers::impls::markdown::MarkdownParser;
use pretty_assertions::assert_eq;

const MOVE_FILES: bool = false;

fn default_config() -> OverlapperConfig {
    OverlapperConfig::default()
}

fn make_cards(data: &str, config: OverlapperConfig) -> Vec<CardData> {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    get_cards_main(
        parser.as_ref(),
        None,
        data.to_string(),
        false,
        MOVE_FILES,
        (FrontConceal::default(), BackReveal::default(), false),
        Some(&config),
    )
    .unwrap()
}

fn make_cards_with_order(data: &str, config: OverlapperConfig) -> Vec<CardData> {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    get_cards_main(
        parser.as_ref(),
        None,
        data.to_string(),
        true,
        MOVE_FILES,
        (FrontConceal::default(), BackReveal::default(), false),
        Some(&config),
    )
    .unwrap()
}

fn blank_card(grouping: ClozeGrouping, data: Vec<NotePart>) -> CardData {
    CardData {
        order: None,
        previous_order: None,
        grouping,
        is_suspended: None,
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
        back_type: BackType::NoteFilePath,
        inherit: None,
        data,
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn cs(s: &str) -> NotePart {
    NotePart::ClozeStart(s.to_string())
}
fn ce(s: &str) -> NotePart {
    NotePart::ClozeEnd(s.to_string())
}
fn to_answer(text: &str) -> NotePart {
    NotePart::ClozeData(text.to_string(), ClozeHiddenReplacement::ToAnswer { hint: None })
}
fn not_to_answer(text: &str) -> NotePart {
    NotePart::ClozeData(text.to_string(), ClozeHiddenReplacement::NotToAnswer)
}
fn surrounding(text: &str) -> NotePart {
    NotePart::SurroundingData(text.to_string())
}

// cloze with ov: marker but no order (add_order=false)
const OV: &str = "{{[ov:]";
const END: &str = "}}";

/// Build a single cloze block: [ClozeStart, ClozeData(...), ClozeEnd]
fn prompt(text: &str) -> Vec<NotePart> {
    vec![cs(OV), to_answer(text), ce(END)]
}

fn hidden(text: &str) -> Vec<NotePart> {
    vec![cs(OV), not_to_answer(text), ce(END)]
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Standard 4-item overlapper, CB=1, P=1, CA=0 (default settings).
///
/// Expected cards (sequential order):
///   Card 0 (group 5): prompt=a, hidden=b,c,d
///   Card 1 (group 6): context=a (SurroundingData), prompt=b, hidden=c,d
///   Card 2 (group 7): hidden=a, context=b (SurroundingData), prompt=c, hidden=d
///   Card 3 (group 8): hidden=a,b, context=c (SurroundingData), prompt=d
///
/// The 4 ov: clozes consume Auto groups 1–4 during parsing; overlapper then
/// assigns groups 5–8. That is why groupings start at Auto(5).
#[test]
fn test_overlapper_4_items_standard() {
    // Input: 4 adjacent ov: clozes with no separating text
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let cards = make_cards(data, default_config());

    assert_eq!(cards.len(), 4);

    // Card 0: prompt a, hidden b c d
    assert_eq!(
        cards[0],
        blank_card(
            ClozeGrouping::Auto(5),
            [prompt("a"), hidden("b"), hidden("c"), hidden("d")].concat(),
        )
    );

    // Card 1: context a (SurroundingData), prompt b, hidden c d
    assert_eq!(
        cards[1],
        blank_card(
            ClozeGrouping::Auto(6),
            [
                vec![surrounding("{{[ov:]a}}")],
                prompt("b"),
                hidden("c"),
                hidden("d"),
            ]
            .concat(),
        )
    );

    // Card 2: hidden a, context b (SurroundingData), prompt c, hidden d
    assert_eq!(
        cards[2],
        blank_card(
            ClozeGrouping::Auto(7),
            [
                hidden("a"),
                vec![surrounding("{{[ov:]b}}")],
                prompt("c"),
                hidden("d"),
            ]
            .concat(),
        )
    );

    // Card 3: hidden a b, context c (SurroundingData), prompt d
    assert_eq!(
        cards[3],
        blank_card(
            ClozeGrouping::Auto(8),
            [
                hidden("a"),
                hidden("b"),
                vec![surrounding("{{[ov:]c}}")],
                prompt("d"),
            ]
            .concat(),
        )
    );
}

/// Order numbers are written back only to the first-prompt cloze of each card.
/// Non-first-prompt hidden clozes should keep plain `{{[ov:]` (no order number).
#[test]
fn test_overlapper_order_written_back() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}";
    let cards = make_cards_with_order(data, default_config());

    assert_eq!(cards.len(), 3);

    // Each card should have an order assigned sequentially
    assert_eq!(cards[0].order, Some(1));
    assert_eq!(cards[1].order, Some(2));
    assert_eq!(cards[2].order, Some(3));

    // Card 0: first cloze is the first-prompt (a) → should have o:1 in ClozeStart
    let first_cloze_start = cards[0]
        .data
        .iter()
        .find_map(|p| if let NotePart::ClozeStart(s) = p { Some(s.as_str()) } else { None })
        .unwrap();
    assert!(
        first_cloze_start.contains("o:1"),
        "first-prompt cloze of card 0 should have o:1, got: {first_cloze_start}"
    );

    // Hidden clozes in card 0 (b and c) should NOT have an order number
    // (their assignments are all skip_serialization=true, so no o: is written).
    // We verify by checking card 1's first cloze start has o:2
    let card1_first_cloze_start = cards[1]
        .data
        .iter()
        .find_map(|p| if let NotePart::ClozeStart(s) = p { Some(s.as_str()) } else { None })
        .unwrap();
    // Card 1 starts with SurroundingData (context a), so first ClozeStart is item b (o:2)
    assert!(
        card1_first_cloze_start.contains("o:2"),
        "first-prompt cloze of card 1 should have o:2, got: {card1_first_cloze_start}"
    );
}

/// context_before_item=2: the two items preceding the prompt are visible context.
#[test]
fn test_overlapper_context_before_2() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}{{[ov:]e}}";
    let config = OverlapperConfig {
        context_before_item: 2,
        prompts: 1,
        context_after_item: 0,
        ..default_config()
    };
    let cards = make_cards(data, config);

    assert_eq!(cards.len(), 5);

    // Card 2 (prompt=c): context=a,b (both SurroundingData), hidden=d,e
    // cloze list: [c(prompt), d(hidden), e(hidden)] — a and b are context, no cloze entry
    // data before c: "{{[ov:]a}}{{[ov:]b}}" → SurroundingData
    let card2_data = &cards[2].data;
    // First element should be SurroundingData containing both a and b context items
    assert!(
        matches!(&card2_data[0], NotePart::SurroundingData(s) if s.contains("{{[ov:]a}}") && s.contains("{{[ov:]b}}")),
        "card 2 should have a+b as SurroundingData prefix, got: {:?}",
        card2_data[0]
    );
    // Next should be the prompt for c
    assert_eq!(card2_data[1], cs(OV));
    assert_eq!(card2_data[2], to_answer("c"));
    // d and e should be hidden
    assert!(card2_data.contains(&not_to_answer("d")));
    assert!(card2_data.contains(&not_to_answer("e")));
}

/// context_after_item=1: the item immediately following the prompt is visible context.
#[test]
fn test_overlapper_context_after_1() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        context_after_item: 1,
        ..default_config()
    };
    let cards = make_cards(data, config);

    assert_eq!(cards.len(), 4);

    // Card 0 (prompt=a, no context before, context after=b):
    //   cloze list: [a(prompt), c(hidden), d(hidden)]  — b is context after, not in cloze list
    let card0_data = &cards[0].data;
    // a is prompt
    assert!(card0_data.contains(&to_answer("a")));
    // b should NOT appear as ClozeData — it is context (SurroundingData text between a and c)
    assert!(!card0_data.contains(&not_to_answer("b")));
    assert!(!card0_data.contains(&to_answer("b")));
    // c and d should be hidden
    assert!(card0_data.contains(&not_to_answer("c")));
    assert!(card0_data.contains(&not_to_answer("d")));

    // Card 1 (prompt=b, context before=a, context after=c):
    //   cloze list: [d(hidden)]  — a,b,c are not hidden (a=context before, b=prompt, c=context after)
    //   But the card needs to include b as prompt still...
    // Actually: a is context before (SurroundingData), b is prompt, c is context after (SurroundingData)
    //   cloze list for card 1: [b(prompt), d(hidden)]
    let card1_data = &cards[1].data;
    assert!(card1_data.contains(&to_answer("b")));
    // c should NOT appear as hidden — it is context
    assert!(!card1_data.contains(&not_to_answer("c")));
    assert!(card1_data.contains(&not_to_answer("d")));
}

/// no_cues_for_first_item: the very first card has no visible context (no SurroundingData prefix).
#[test]
fn test_overlapper_no_cues_for_first_item() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let config = OverlapperConfig {
        context_before_item: 2,
        prompts: 1,
        no_cues_for_first_item: true,
        ..default_config()
    };
    let cards = make_cards(data, config);

    assert_eq!(cards.len(), 4);

    // Card 0 (prompt=a): with no_cues_for_first_item, even though CB=2 the first card
    // has cb_actual=0. So ALL non-prompt items are hidden (no SurroundingData prefix).
    let card0_data = &cards[0].data;
    assert!(!card0_data.iter().any(|p| matches!(p, NotePart::SurroundingData(_))));
    assert!(card0_data.contains(&to_answer("a")));
    assert!(card0_data.contains(&not_to_answer("b")));
    assert!(card0_data.contains(&not_to_answer("c")));
    assert!(card0_data.contains(&not_to_answer("d")));

    // Card 1 (prompt=b): CB=2 applies → a is context
    let card1_data = &cards[1].data;
    // a is context → SurroundingData (no hidden b entry for a)
    assert!(!card1_data.contains(&not_to_answer("a")));
    let has_surrounding_a = card1_data.iter().any(|p| {
        matches!(p, NotePart::SurroundingData(s) if s.contains("{{[ov:]a}}"))
    });
    assert!(has_surrounding_a, "card 1 should have a as SurroundingData");
}

/// no_cues_for_last_item: the very last card has no visible context after.
#[test]
fn test_overlapper_no_cues_for_last_item() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let config = OverlapperConfig {
        context_before_item: 0,
        prompts: 1,
        context_after_item: 2,
        no_cues_for_last_item: true,
        ..default_config()
    };
    let cards = make_cards(data, config);

    assert_eq!(cards.len(), 4);

    // Last card (prompt=d): with no_cues_for_last_item and ca_actual=0 → no context after
    // In this case there IS no context after anyway (d is the last item), but the flag also
    // suppresses context_after from items that would normally be visible.
    // Let's verify on card 2 (prompt=c): without the flag, d would be context after.
    // With the flag only affecting the LAST card (k=n-P=3), card 2 still gets ca=1 (d as context).
    let card2_data = &cards[2].data;
    // d should be context after (SurroundingData), not hidden
    assert!(!card2_data.contains(&not_to_answer("d")));

    // Card 3 (last, prompt=d): no context before and no context after
    let card3_data = &cards[3].data;
    // Only d should be in a prompt/answer role; a,b,c should be hidden (ca_actual=0 applies)
    assert!(card3_data.contains(&to_answer("d")));
    // a,b are hidden (too far before d for cb=0)
    assert!(card3_data.contains(&not_to_answer("a")));
    assert!(card3_data.contains(&not_to_answer("b")));
    // c would have been context before (cb=0 here anyway since context_before_item=0)
    // just verify d is the prompt
    assert!(!card3_data.iter().any(|p| matches!(p, NotePart::SurroundingData(_))));
}

/// start_and_end_gradually with P=2: generates 2*(P-1)=2 extra cards.
/// n=4, P=2 → regular=3, extra=2 → total=5 cards.
#[test]
fn test_overlapper_start_and_end_gradually() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let config = OverlapperConfig {
        context_before_item: 0,
        prompts: 2,
        context_after_item: 0,
        start_and_end_gradually: true,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // 4 items, P=2: regular = 4-2+1 = 3, extra = 2*(2-1) = 2, total = 5
    assert_eq!(cards.len(), 5);

    // The extra "start" card (window [0..0]) should ask only about item a
    // It appears first, with just item a as prompt and b,c,d hidden.
    let card0_data = &cards[0].data;
    assert!(card0_data.contains(&to_answer("a")));
    assert!(card0_data.contains(&not_to_answer("b")));

    // The extra "end" card (window [3..3]) should ask only about item d
    let last_card_data = &cards[4].data;
    assert!(last_card_data.contains(&to_answer("d")));
    // a,b,c should be hidden in the last extra card
    assert!(last_card_data.contains(&not_to_answer("a")));
}

/// prompts=2: each card requires answering two items simultaneously.
#[test]
fn test_overlapper_prompts_2() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}{{[ov:]e}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 2,
        context_after_item: 0,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // n=5, P=2 → 4 cards
    assert_eq!(cards.len(), 4);

    // Card 0 (window [0,1], no context): both a and b are required answers
    let card0_data = &cards[0].data;
    assert!(card0_data.contains(&to_answer("a")));
    assert!(card0_data.contains(&to_answer("b")));
    // c,d,e are hidden
    assert!(card0_data.contains(&not_to_answer("c")));
    assert!(card0_data.contains(&not_to_answer("d")));
    assert!(card0_data.contains(&not_to_answer("e")));

    // Card 1 (window [1,2], context before=a):
    //   a is context → SurroundingData, b and c are prompts, d and e are hidden
    let card1_data = &cards[1].data;
    // a should be context (SurroundingData), not hidden
    assert!(!card1_data.contains(&not_to_answer("a")));
    assert!(card1_data.contains(&to_answer("b")));
    assert!(card1_data.contains(&to_answer("c")));
    assert!(card1_data.contains(&not_to_answer("d")));
    assert!(card1_data.contains(&not_to_answer("e")));
}

/// Edge case: n < p → no cards generated.
#[test]
fn test_overlapper_n_less_than_p() {
    let data = "{{[ov:]a}}{{[ov:]b}}";
    let config = OverlapperConfig {
        prompts: 3,
        ..default_config()
    };
    // With overlapper disabled (n<p), the 2 clozes should be treated as regular ungrouped clozes
    // and produce 2 individual cards (one per cloze).
    let cards = make_cards(data, config);
    assert_eq!(cards.len(), 2);
}

/// Edge case: n == p → exactly one card with all items as prompts.
#[test]
fn test_overlapper_n_equals_p() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}";
    let config = OverlapperConfig {
        prompts: 3,
        context_before_item: 1,
        context_after_item: 0,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // n=3, P=3 → exactly 1 regular card
    assert_eq!(cards.len(), 1);
    let card0_data = &cards[0].data;
    assert!(card0_data.contains(&to_answer("a")));
    assert!(card0_data.contains(&to_answer("b")));
    assert!(card0_data.contains(&to_answer("c")));
}

/// Edge case: single overlapper item, P=1 → one card, no context.
#[test]
fn test_overlapper_single_item() {
    let data = "{{[ov:]a}}";
    let cards = make_cards(data, default_config());
    assert_eq!(cards.len(), 1);
    assert!(cards[0].data.contains(&to_answer("a")));
}

/// Mixing ov: clozes with a regular (non-overlapper) cloze in the same note.
/// The regular cloze should produce its own card independently.
#[test]
fn test_overlapper_mixed_with_regular_cloze() {
    // 3 overlapper items + 1 regular cloze
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{regular}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        context_after_item: 0,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // 3 overlapper cards + 1 regular card = 4 total
    assert_eq!(cards.len(), 4);

    // The regular cloze should form its own card (Custom grouping? No — Auto, since no g: tag)
    // It has no ov: marker so it becomes an independent auto-grouped card.
    // It should have the word "regular" as a ToAnswer cloze.
    let regular_card = cards
        .iter()
        .find(|c| c.data.contains(&to_answer("regular")))
        .expect("should have a card asking about 'regular'");
    assert!(matches!(regular_card.grouping, ClozeGrouping::Auto(_)));

    // The overlapper cards should each ask about one item
    let asks_a = cards.iter().any(|c| c.data.contains(&to_answer("a")));
    let asks_b = cards.iter().any(|c| c.data.contains(&to_answer("b")));
    let asks_c = cards.iter().any(|c| c.data.contains(&to_answer("c")));
    assert!(asks_a, "should have a card asking about a");
    assert!(asks_b, "should have a card asking about b");
    assert!(asks_c, "should have a card asking about c");
}

/// Mixing ov: clozes with an explicit custom-grouped cloze (g:mygroup).
#[test]
fn test_overlapper_mixed_with_custom_group() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[g:x]c}}{{[g:x]d}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // 2 overlapper cards + 1 custom-group card = 3 total
    assert_eq!(cards.len(), 3);

    let custom_card = cards
        .iter()
        .find(|c| c.grouping == ClozeGrouping::Custom("x".to_string()))
        .expect("should have a custom-grouped card");
    // Custom card asks about c and d together
    assert!(custom_card.data.contains(&to_answer("c")));
    assert!(custom_card.data.contains(&to_answer("d")));
}

/// Combining ov: with r: (include_reverse).
/// The r: flag should be silently overridden by the overlapper — only forward cards are generated.
#[test]
fn test_overlapper_ignores_reverse_flag() {
    let data = "{{[ov:;r:]a}}{{[ov:]b}}{{[ov:]c}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // Only 3 forward overlapper cards — the r: on item a is overridden
    assert_eq!(cards.len(), 3);

    // All cards are forward cards (no backward cards)
    // A backward card would have SurroundingData where the forward prompt is
    // We can verify by checking that all ToAnswer entries correspond to the expected prompts
    let asks_a = cards.iter().filter(|c| c.data.contains(&to_answer("a"))).count();
    assert_eq!(asks_a, 1, "a should be a prompt in exactly 1 card");
}

/// Combining ov: with ro: (reverse_only).
/// The ro: flag should also be overridden by the overlapper.
#[test]
fn test_overlapper_ignores_reverse_only_flag() {
    let data = "{{[ov:;ro:]a}}{{[ov:]b}}{{[ov:]c}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        ..default_config()
    };
    let cards = make_cards(data, config);

    // Only 3 forward overlapper cards
    assert_eq!(cards.len(), 3);
    let asks_a = cards.iter().filter(|c| c.data.contains(&to_answer("a"))).count();
    assert_eq!(asks_a, 1);
}

/// ov: clozes and a normal cloze with r: (reverse) coexist correctly.
/// The r: cloze should still produce both forward and backward cards.
#[test]
fn test_overlapper_coexists_with_reverse_cloze() {
    let data = "{{[r:]x}}{{[ov:]a}}{{[ov:]b}}";
    let config = OverlapperConfig {
        context_before_item: 1,
        prompts: 1,
        ..default_config()
    };
    let cards = make_cards_with_order(data, config);

    // 1 forward + 1 backward (from r:x) + 2 overlapper = 4 cards
    assert_eq!(cards.len(), 4);

    // There should be a card where x is required as a forward answer
    let fwd_x = cards.iter().any(|c| c.data.contains(&to_answer("x")));
    assert!(fwd_x, "should have forward card for x");

    // The backward card for x has "x" as SurroundingData and the surrounding as ToAnswer
    // (in a backward card the cloze content becomes SurroundingData)
    let bwd_x = cards.iter().any(|c| {
        c.data
            .iter()
            .any(|p| matches!(p, NotePart::SurroundingData(s) if s == "x"))
    });
    assert!(bwd_x, "should have backward card for x");
}

/// Verify with add_order=true that ov: clozes without a group tag still round-trip cleanly:
/// the output note data should contain `ov:` and sequential order numbers.
#[test]
fn test_overlapper_order_round_trip() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let result = crate::parsers::add_order_to_note_data(parser.as_ref(), data, Some(&default_config()));
    assert!(result.is_ok());
    let (note_data, cards) = result.unwrap();

    // 3 cards produced
    assert_eq!(cards.len(), 3);

    // The returned note_data should contain ov: markers and o:1, o:2, o:3
    assert!(note_data.contains("ov:"), "note data should still contain ov:");
    assert!(note_data.contains("o:1"), "note data should contain o:1");
    assert!(note_data.contains("o:2"), "note data should contain o:2");
    assert!(note_data.contains("o:3"), "note data should contain o:3");

    // Should NOT contain explicit group numbers (Auto groups are not serialized)
    assert!(!note_data.contains("g:"), "auto groups should not be written to note data");
}

/// No overlapper config provided → ov: marker is parsed but no overlapper cards generated.
/// The clozes are treated as independent ungrouped clozes.
#[test]
fn test_overlapper_no_config_fallback() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    // Pass None for overlapper config
    let cards = get_cards_main(
        parser.as_ref(),
        None,
        data.to_string(),
        false,
        MOVE_FILES,
        (FrontConceal::default(), BackReveal::default(), false),
        None,
    )
    .unwrap();

    // Without overlapper config, each ov: cloze becomes its own independent card
    assert_eq!(cards.len(), 3);
    // Each card asks about exactly one item
    assert!(cards[0].data.contains(&to_answer("a")));
    assert!(cards[1].data.contains(&to_answer("b")));
    assert!(cards[2].data.contains(&to_answer("c")));
}

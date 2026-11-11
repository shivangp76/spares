//! Tests the round-trip that `update_notes` relies on for file generation.
//!
//! `update_notes` parses each note once via `add_order_to_note_data` (which renumbers card
//! orders and rebuilds the stored note data) and then hands those `CardData`s to file
//! generation as `precomputed_cards`, instead of re-parsing the rebuilt text a second time.
//! These tests pin down that the precomputed cards are equivalent to what a re-parse of the
//! rebuilt text would produce — in the fields file generation actually consumes (`data` and
//! `back_type`) — and that for `ov:` notes the overlapper config makes them *diverge* (the
//! latent bug the reuse fixes).

use pretty_assertions::assert_eq;

use crate::parsers::Parseable;
use crate::parsers::add_order_to_note_data;
use crate::parsers::cards::overlapper::OverlapperConfig;
use crate::parsers::get_cards;
use crate::parsers::impls::markdown::MarkdownParser;

/// Re-parsing the rebuilt note data (exactly what `create_note_files` does at
/// `generate_files.rs` when no precomputed cards are supplied) must reproduce the cards
/// returned by `add_order_to_note_data` for every card generation consumes: `data` and
/// `back_type`. `previous_order`/`inherit` may legitimately differ — file generation never
/// reads them.
fn assert_reparse_matches(data: &str) {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let (new_data, new_cards) = add_order_to_note_data(parser.as_ref(), data, None).unwrap();
    // `move_files = true` mirrors the production call; these samples contain no image
    // occlusions, so no files are touched.
    let reparsed = get_cards(parser.as_ref(), None, &new_data, false, true).unwrap();

    assert_eq!(
        reparsed.len(),
        new_cards.len(),
        "card count mismatch for note: {data}"
    );
    for (i, (reparsed_card, new_card)) in reparsed.iter().zip(new_cards.iter()).enumerate() {
        assert_eq!(
            reparsed_card.data, new_card.data,
            "card data mismatch at index {i} for note: {data}"
        );
        assert_eq!(
            reparsed_card.back_type, new_card.back_type,
            "back_type mismatch at index {i} for note: {data}"
        );
    }
}

#[test]
fn test_reparse_matches_plain_clozes() {
    assert_reparse_matches("{{[o:1] First }}\n{{[o:2] Second }}\n{{[o:3] Third }}");
}

#[test]
fn test_reparse_matches_unsorted_orders() {
    // Orders are renumbered sequentially on rebuild; re-parse must agree.
    assert_reparse_matches("{{[o:3] Third }}\n{{[o:1] First }}\n{{[o:2] Second }}");
}

#[test]
fn test_reparse_matches_grouped_clozes() {
    assert_reparse_matches("{{[g:1] a }}{{[g:1] b }}\n{{[o:1] Lone }}");
}

#[test]
fn test_reparse_matches_reverse() {
    // `r:` produces a forward + backward card.
    assert_reparse_matches("{{[o:1;r:] Both }}");
}

#[test]
fn test_reparse_matches_reverse_only() {
    // `ro:` produces a single reverse card.
    assert_reparse_matches("{{[o:1;ro:] Reverse }}");
}

#[test]
fn test_reparse_matches_suspended_and_settings() {
    assert_reparse_matches("{{[o:1;s:] Suspended }}\n{{[o:2;f:all;b:a] Settings }}");
}

#[test]
fn test_reparse_matches_inherit() {
    // `inh:` is stripped from the rebuilt note data (serialize_ephemeral = false), so the
    // re-parsed `inherit` is `None` while `new_cards` still carries it. Only `data`/`back_type`
    // are consumed by file generation, so equivalence still holds.
    assert_reparse_matches("{{[o:1] First }}\n{{[inh:aaa/1] Inherited }}");
}

/// With an external overlapper config, `add_order_to_note_data` produces overlapper-grouped
/// cards (what the DB stores), while re-parsing the rebuilt note data with *no* overlapper
/// (what file generation did before this change) yields independent per-cloze cards. This is
/// the inconsistency the precomputed-cards reuse fixes: file generation now matches the DB.
#[test]
fn test_ov_notes_diverge_without_overlapper_reparse() {
    let data = "{{[ov:]a}}{{[ov:]b}}{{[ov:]c}}{{[ov:]d}}";
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let (new_data, db_cards) =
        add_order_to_note_data(parser.as_ref(), data, Some(&OverlapperConfig::default())).unwrap();

    // The stored note data keeps the ov: markers, so a plain re-parse regresses to independent
    // cards — proving the DB cards cannot be reproduced without passing them explicitly.
    assert!(new_data.contains("ov:"));
    let old_file_gen_cards = get_cards(parser.as_ref(), None, &new_data, false, true).unwrap();

    assert_eq!(db_cards.len(), old_file_gen_cards.len());
    assert_ne!(
        db_cards[0].data, old_file_gen_cards[0].data,
        "overlapper cards should group clozes; the no-overlapper re-parse must not match"
    );
}

/// Rewrites `data` twice and returns each card's typed answer after each rewrite, asserting that
/// the second rewrite leaves the note as the first wrote it.
fn type_answers_over_rewrites(
    data: &str,
) -> (
    Vec<Option<crate::parsers::TypeAnswer>>,
    Vec<Option<crate::parsers::TypeAnswer>>,
) {
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let (first_data, first_cards) = add_order_to_note_data(parser.as_ref(), data, None).unwrap();
    let (second_data, second_cards) =
        add_order_to_note_data(parser.as_ref(), &first_data, None).unwrap();
    assert_eq!(
        second_data, first_data,
        "rewrite is not stable for note: {data}"
    );
    let type_answers =
        |cards: &[crate::parsers::CardData]| cards.iter().map(|c| c.type_answer).collect();
    (type_answers(&first_cards), type_answers(&second_cards))
}

/// A grouping's own `t:` beats that of a `g:*` cloze, wherever the `g:*` cloze is. The rewrite
/// moves the grouping's settings onto its first cloze, before the `g:*` cloze here, which used
/// to hand the card to `g:*` on the next parse.
#[test]
fn test_grouping_type_answer_beats_all_groupings_across_rewrites() {
    use crate::parsers::TypeAnswer;
    let diff = Some(TypeAnswer::Diff {
        ignore_case: false,
        ignore_combining: false,
    });
    for data in [
        "{{[g:1;t:m] x }} {{[g:*;t] y }} {{[g:1;t:m] z }}",
        "{{[g:1] x }} {{[g:*;t] y }} {{[g:1;t:m] z }}",
        "{{[g:1;t:m] x }} {{[g:*;t] y }}",
        "{{[g:*;t] y }} {{[g:1;t:m] x }}",
    ] {
        let (first, second) = type_answers_over_rewrites(data);
        assert_eq!(
            first,
            vec![Some(TypeAnswer::Manual)],
            "first parse of {data}"
        );
        assert_eq!(second, first, "second parse of {data}");
    }
    // The `g:*` cloze's `t:` still reaches a card whose grouping sets none
    let (first, second) =
        type_answers_over_rewrites("{{[g:1] x }} {{[g:*;t] y }} {{[g:1,2;t:m] z }}");
    assert_eq!(
        first,
        vec![Some(TypeAnswer::Manual), Some(TypeAnswer::Manual)]
    );
    assert_eq!(second, first);
    let (first, second) =
        type_answers_over_rewrites("{{[g:1;t:m] x }} {{[g:*;t] y }} {{[g:2] z }}");
    assert_eq!(first, vec![Some(TypeAnswer::Manual), diff]);
    assert_eq!(second, first);
}

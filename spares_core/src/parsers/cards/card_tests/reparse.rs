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
    assert_reparse_matches("{{[o:1] First }}\n{{[inh:1/1] Inherited }}");
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

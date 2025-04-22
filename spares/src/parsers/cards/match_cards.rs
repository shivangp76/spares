use crate::{CardErrorKind, Error, LibraryError};
use itertools::Itertools;
use std::collections::HashSet;

#[derive(Debug)]
pub struct MatchCardsResult {
    pub move_card_indices: Vec<(usize, usize)>,
    pub delete_card_indices: Vec<usize>,
    pub create_card_indices: Vec<usize>,
}

/// This function determines how cards were changed when a note changes. A note is rendered with its card indices in sequential order. This means that `old_cards` will be similar to `[Some(1), Some(2), Some(3)]`. New cards will contain the new indices. Consider the following example:
/// ```
/// use spares::parsers::{match_cards, MatchCardsResult};
///
/// let old_cards = vec![Some(1), Some(2), Some(3), Some(4)];
/// let new_cards = vec![Some(1), Some(3), None, None, None, Some(2)];
/// let match_cards_res = match_cards(&old_cards, &new_cards);
/// assert!(match_cards_res.is_ok());
/// let MatchCardsResult {
///     move_card_indices,
///     delete_card_indices,
///     create_card_indices,
/// } = match_cards_res.unwrap();
/// assert_eq!(move_card_indices, vec![(3, 2), (2, 6)]);
/// assert_eq!(delete_card_indices, vec![4]);
/// assert_eq!(create_card_indices, vec![3, 4, 5]);
/// ```
/// This function parses the following:
/// 1. By examining the position of `None` in `new_cards` it determines that new cards with indices 3, 4, and 5 need to be created.
/// 2. The card with index 3 is moved to the index 2. The card with index 2 is moved to the index 6.
/// 3. The card with index 4 needs to be deleted since it is not present in `new_cards`.
/// 4. The card with index 1 is left unchanged since its position in `old_cards` is the same as its position in `new_cards`.
pub fn match_cards(
    old_cards: &[Option<usize>],
    new_cards: &[Option<usize>],
) -> Result<MatchCardsResult, Error> {
    // Validate data
    let old_cards_contain_order = old_cards.iter().all(|x| x.is_some());
    if !old_cards_contain_order {
        return Err(Error::Library(LibraryError::Card(
            CardErrorKind::InvalidInput(format!(
                "Current cards do not all contain order. Current data is incorrect. Found `{}`",
                old_cards.iter().flatten().join(", "),
            )),
        )));
    }
    let old_cards_contain_expected_order = old_cards
        .iter()
        .flatten()
        .enumerate()
        .all(|(i, x)| i + 1 == *x);
    if !old_cards_contain_expected_order {
        return Err(Error::Library(LibraryError::Card(
            CardErrorKind::InvalidInput(format!(
                "Current cards do not all contain their expected order. Current data is incorrect. Found orders `{}` when expecting sequential orders `{}`",
                old_cards.iter().flatten().join(", "),
                old_cards
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, _)| i + 1)
                    .join(", ")
            )),
        )));
    }
    let new_cards_contain_expected_order = new_cards
        .iter()
        .flatten()
        .all(|x| 1 <= *x && *x <= old_cards.len());
    if !new_cards_contain_expected_order {
        let new_cards_order_str = new_cards
            .iter()
            .flatten()
            .collect::<Vec<_>>()
            .into_iter()
            .join(", ");
        return Err(Error::Library(LibraryError::Card(
            CardErrorKind::InvalidInput(format!(
                "New cards do not all contain a valid order. They must all be within the range [1, {}] (inclusive), but the new cards have order `{}`.",
                old_cards.len(),
                new_cards_order_str
            )),
        )));
    }

    let mut create_card_indices: Vec<usize> = Vec::new();
    let mut same_indices: Vec<usize> = Vec::new();
    let mut move_card_indices: Vec<(usize, usize)> = Vec::new();

    let old_cards_padded = old_cards.iter().copied().chain(std::iter::repeat(None));
    for (i, (old_card_order, new_card_order)) in old_cards_padded
        .into_iter()
        .zip(new_cards.iter())
        .enumerate()
    {
        let new_index = i + 1;
        match (old_card_order, *new_card_order) {
            (Some(old_order), Some(new_order)) => {
                if old_order == new_order {
                    same_indices.push(old_order);
                } else {
                    // Move card
                    move_card_indices.push((new_order, old_order));
                }
            }
            (None, None) => {
                // Create new card
                create_card_indices.push(new_index);
            }
            (None, Some(new_order)) => {
                // Move card
                move_card_indices.push((new_order, new_index));
            }
            (Some(old_order), None) => {
                // Create new card
                create_card_indices.push(old_order);
            }
        }
    }
    let expected_indices = old_cards.iter().flatten().copied().collect::<HashSet<_>>();
    let present_indices = move_card_indices
        .iter()
        .map(|(t, _f)| t)
        .collect::<Vec<_>>()
        .into_iter()
        .chain(same_indices.iter())
        .copied()
        .collect::<HashSet<_>>();
    let delete_card_indices = expected_indices
        .difference(&present_indices)
        .copied()
        .collect::<Vec<_>>();
    Ok(MatchCardsResult {
        move_card_indices,
        delete_card_indices,
        create_card_indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_cards_basic() {
        let old_cards = vec![Some(1), Some(2), Some(3)];
        let new_cards = vec![Some(1), Some(2), Some(3)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_ok());
        if let Ok(MatchCardsResult {
            move_card_indices,
            delete_card_indices,
            create_card_indices,
        }) = match_cards_res
        {
            assert!(move_card_indices.is_empty());
            assert!(delete_card_indices.is_empty());
            assert!(create_card_indices.is_empty());
        }
    }

    #[test]
    fn test_match_cards_advanced() {
        let old_cards = vec![Some(1), Some(2), Some(3), Some(4)];
        let new_cards = vec![Some(1), Some(3), None, None, None, Some(2)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_ok());
        if let Ok(MatchCardsResult {
            move_card_indices,
            delete_card_indices,
            create_card_indices,
        }) = match_cards_res
        {
            assert_eq!(move_card_indices, vec![(3, 2), (2, 6)]);
            assert_eq!(delete_card_indices, vec![4]);
            assert_eq!(create_card_indices, vec![3, 4, 5]);
        }
    }

    #[test]
    fn test_match_cards_invalid_1() {
        let old_cards = vec![Some(1), Some(2)];
        let new_cards = vec![Some(1), Some(3)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_err());
    }

    #[test]
    fn test_match_cards_invalid_2() {
        let old_cards = vec![Some(1), None];
        let new_cards = vec![Some(1), Some(2)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_err());
    }

    #[test]
    fn test_match_cards_invalid_3() {
        let old_cards = vec![Some(1), Some(3)];
        let new_cards = vec![Some(1), Some(2)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_err());
    }

    #[test]
    fn test_match_cards_swap() {
        let old_cards = vec![Some(1), Some(2)];
        let new_cards = vec![Some(2), Some(1)];
        let match_cards_res = match_cards(&old_cards, &new_cards);
        assert!(match_cards_res.is_ok());
        if let Ok(MatchCardsResult {
            move_card_indices,
            delete_card_indices,
            create_card_indices,
        }) = match_cards_res
        {
            assert_eq!(move_card_indices, vec![(2, 1), (1, 2)]);
            assert!(delete_card_indices.is_empty());
            assert!(create_card_indices.is_empty());
        }
    }
}

use crate::{DelimiterErrorKind, parsers::RegexMatch};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use indexmap::IndexMap;
use itertools::Itertools;
use std::{collections::HashSet, hash::Hash};

pub fn change_offset(original: &mut usize, change: i64) {
    let original_int = i64::try_from(*original).unwrap();
    if change < 0 {
        assert!(change.abs() <= original_int);
    }
    *original = usize::try_from(original_int + change).unwrap();
}

pub fn get_start_end_local_date(requested_date: &DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let lower_limit = Local
        .from_local_datetime(
            &requested_date
                .with_timezone(&Local)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .unwrap()
        .to_utc();
    let upper_limit = Local
        .from_local_datetime(
            &requested_date
                .with_timezone(&Local)
                .date_naive()
                .and_hms_opt(23, 59, 59)
                .unwrap(),
        )
        .unwrap()
        .to_utc();
    (lower_limit, upper_limit)
}

pub fn intersect<T: Eq + std::hash::Hash + Clone>(vec1: &[T], vec2: &[T]) -> Vec<T> {
    let set1: HashSet<_> = vec1.iter().cloned().collect();
    let set2: HashSet<_> = vec2.iter().cloned().collect();
    set1.intersection(&set2).cloned().collect()
}

/// Merge 2 sorted lists into a new sorted list
pub fn merge_by_key<T, K, F>(list1: &[T], list2: &[T], key: F) -> Vec<T>
where
    T: Clone,
    K: Ord,
    F: Fn(&T) -> K,
{
    let mut merged = Vec::with_capacity(list1.len() + list2.len());
    let mut i = 0;
    let mut j = 0;

    while i < list1.len() && j < list2.len() {
        if key(&list1[i]) <= key(&list2[j]) {
            merged.push(list1[i].clone());
            i += 1;
        } else {
            merged.push(list2[j].clone());
            j += 1;
        }
    }

    // Push remaining elements from list1
    while i < list1.len() {
        merged.push(list1[i].clone());
        i += 1;
    }

    // Push remaining elements from list2
    while j < list2.len() {
        merged.push(list2[j].clone());
        j += 1;
    }

    merged
}

pub fn is_monotonic_increasing<A>(vec: &[A]) -> bool
where
    A: Ord,
{
    vec.iter().tuple_windows().all(|(cur, next)| cur < next)
}

// [10, 40, 33, 20];
// rev: [20, 33, 40, 10];
// split_inclusive (if x == 33): [20, 33] [40, 10];
// rev outer: [40, 10] [20, 33]
// rev inner: [10, 40] [33, 20]
/// Similar to `std::slice::split_inclusive`, but the matched element is contained at the end of the following subslice.
pub fn split_inclusive_following<T, F>(slice: &[T], pred: F) -> Vec<Vec<T>>
where
    T: Clone,
    F: Fn(&T) -> bool,
{
    slice
        .iter()
        .rev()
        .collect::<Vec<_>>()
        .split_inclusive(|x| pred(x))
        .rev()
        .map(|x| x.iter().rev().map(|x| (*x).clone()).collect::<Vec<_>>())
        .collect::<Vec<_>>()
}

pub trait FractionalDays {
    fn num_fractional_days(&self) -> f64;
    fn fractional_days(fractional_days: f64) -> Self;
}

impl FractionalDays for Duration {
    fn num_fractional_days(&self) -> f64 {
        let seconds_in_a_day = 24.0 * 3600.0;
        f64::from(self.num_seconds() as i32) / seconds_in_a_day
    }

    fn fractional_days(fractional_days: f64) -> Self {
        Duration::seconds((fractional_days * 24. * 60. * 60.).round() as i64)
    }
}

pub trait GroupByInsertion<A, B> {
    /// Groups the provided elements by A, sorted by the first presence of A. Thus, this is deterministic. Essentially, this is `.into_group_map()` provided by `itertools` if it were to return an `IndexMap` (from the `indexmap` crate) instead of a `HashMap`.
    fn into_group_by_insertion(self) -> Vec<(A, Vec<B>)>;
}

impl<A, B, I> GroupByInsertion<A, B> for I
where
    A: Hash + Eq,
    I: IntoIterator<Item = (A, B)>,
{
    fn into_group_by_insertion(self) -> Vec<(A, Vec<B>)> {
        let mut grouping: IndexMap<A, Vec<B>> = IndexMap::new();
        for (key, item) in self {
            grouping.entry(key).or_default().push(item);
        }
        grouping.into_iter().collect::<Vec<_>>()
    }
}

/// Finds the corresponding end delimiter in the remainder of a string.
/// For example, if you had the string "(())" and you wanted to find the matching end delimiter for the first open parenthesis, then you would pass in "())" to get a result of `Ok(2)` which is the index of the corresponding end parenthesis.
pub fn find_pair(
    data: &str,
    start_delim: char,
    end_delim: char,
) -> Result<usize, DelimiterErrorKind> {
    let mut start_delim_counter = 0;
    for (i, c) in data.chars().enumerate() {
        if c == start_delim {
            start_delim_counter += 1;
        } else if c == end_delim {
            if start_delim_counter == 0 {
                return Ok(i);
            }
            start_delim_counter -= 1;
        }
    }
    if start_delim_counter > 0 {
        Err(DelimiterErrorKind::UnequalMatches {
            src: data.to_string(),
        })
    } else {
        Err(DelimiterErrorKind::EndMatchNotFound {
            src: data.to_string(),
        })
    }
}

/// Returns a vector with the same length as `start_matches`. The result is sorted by the left endpoint.
///
/// # Errors
///
/// Will error if `start_matches` and `end_matches` cannot be lined up properly.
pub fn find_pairs(
    data: &str,
    start_matches: &[RegexMatch],
    end_matches: &Vec<RegexMatch>,
) -> Result<Vec<(RegexMatch, RegexMatch)>, DelimiterErrorKind> {
    let mut result: Vec<(RegexMatch, RegexMatch)> = Vec::new();
    let mut start_matches = start_matches.to_owned();

    for end_match in end_matches {
        let mut matching_start_match_idx = None;
        let mut start_matches_iter = start_matches.iter().enumerate().peekable();
        while let Some((idx, start_match)) = start_matches_iter.peek() {
            if start_match.match_range.start > end_match.match_range.start {
                break;
            }
            matching_start_match_idx = Some(*idx);
            start_matches_iter.next();
        }
        let matching_start_match_idx =
            matching_start_match_idx.ok_or(DelimiterErrorKind::StartMatchNotFound {
                src: data.to_string(),
            })?;

        // Found match
        let matching_start_match = start_matches.remove(matching_start_match_idx);
        result.push((matching_start_match, end_match.clone()));
    }

    result.sort_by_key(|a| a.0.match_range.start);

    Ok(result)
}

#[allow(clippy::cast_precision_loss)]
pub fn mean(vec: &[f64]) -> Option<f64> {
    if vec.is_empty() {
        return None;
    }
    let sum: f64 = vec.iter().sum();
    let count = vec.len() as f64;
    Some(sum / count)
}

pub fn parse_list(data: &str) -> Vec<String> {
    data.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
}

pub fn to_title_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;

    for c in s.chars() {
        if c == '_' || c == '-' {
            result.push(' ');
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn test_into_group_by_insertion() {
        let input = vec![("a", 1), ("b", 2), ("a", 3), ("c", 4), ("b", 5)];
        assert_eq!(
            input.into_iter().into_group_by_insertion(),
            vec![("a", vec![1, 3]), ("b", vec![2, 5]), ("c", vec![4])]
        );
    }

    #[test]
    fn test_find_pair_ok() {
        let tests = vec![(")(())", 0), ("(()))", 4)];
        let start_delim = '(';
        let end_delim = ')';
        for (data, expected) in tests {
            let pairs = find_pair(data, start_delim, end_delim);
            assert!(pairs.is_ok());
            assert_eq!(pairs.unwrap(), expected);
        }
    }

    #[test]
    fn test_find_pair_err() {
        let tests = vec!["()", "( () )", "(()"];
        let start_delim = '(';
        let end_delim = ')';
        for data in tests {
            let pairs = find_pair(data, start_delim, end_delim);
            assert!(pairs.is_err());
        }
    }

    #[test]
    fn test_find_pairs_1() {
        // 0 1 2 3 4 5
        // ( ( ) ( ) )
        let start_matches = vec![(0, 0), (1, 1), (3, 3)];
        let end_matches = vec![(2, 2), (4, 4), (5, 5)];
        let start_matches = start_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let end_matches = end_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let result = find_pairs("(()())", &start_matches, &end_matches);
        assert!(result.is_ok());
        if let Ok(result) = result {
            let expected = vec![((0, 0), (5, 5)), ((1, 1), (2, 2)), ((3, 3), (4, 4))]
                .into_iter()
                .map(|((ss, se), (es, ee))| {
                    (
                        RegexMatch {
                            match_range: (ss..se),
                            capture_range: (ss..se),
                        },
                        RegexMatch {
                            match_range: (es..ee),
                            capture_range: (es..ee),
                        },
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_find_pairs_2() {
        // 0 1 2 3 4 5 6 7
        // ( ( ) ) ( ) ( )
        let start_matches = vec![(0, 0), (1, 1), (4, 4), (6, 6)];
        let end_matches = vec![(2, 2), (3, 3), (5, 5), (7, 7)];
        let start_matches = start_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let end_matches = end_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let result = find_pairs("(())()()", &start_matches, &end_matches);
        assert!(result.is_ok());
        if let Ok(result) = result {
            let expected = vec![
                ((0, 0), (3, 3)),
                ((1, 1), (2, 2)),
                ((4, 4), (5, 5)),
                ((6, 6), (7, 7)),
            ]
            .into_iter()
            .map(|((ss, se), (es, ee))| {
                (
                    RegexMatch {
                        match_range: (ss..se),
                        capture_range: (ss..se),
                    },
                    RegexMatch {
                        match_range: (es..ee),
                        capture_range: (es..ee),
                    },
                )
            })
            .collect::<Vec<_>>();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_find_pairs_3() {
        // 0 1 2 3 4 5 6 7 8 9
        // ( ( ( ) ) ( ) ) ( )
        let start_matches = vec![(0, 0), (1, 1), (2, 2), (5, 5), (8, 8)];
        let end_matches = vec![(3, 3), (4, 4), (6, 6), (7, 7), (9, 9)];
        let start_matches = start_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let end_matches = end_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let result = find_pairs("((())())()", &start_matches, &end_matches);
        assert!(result.is_ok());
        if let Ok(result) = result {
            let expected = vec![
                ((0, 0), (7, 7)),
                ((1, 1), (4, 4)),
                ((2, 2), (3, 3)),
                ((5, 5), (6, 6)),
                ((8, 8), (9, 9)),
            ]
            .into_iter()
            .map(|((ss, se), (es, ee))| {
                (
                    RegexMatch {
                        match_range: (ss..se),
                        capture_range: (ss..se),
                    },
                    RegexMatch {
                        match_range: (es..ee),
                        capture_range: (es..ee),
                    },
                )
            })
            .collect::<Vec<_>>();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_find_pairs_4() {
        // 0 1
        // ) (
        let start_matches = vec![(1, 1)];
        let end_matches = vec![(0, 0)];
        let start_matches = start_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let end_matches = end_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let result = find_pairs(")(", &start_matches, &end_matches);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_pairs_5() {
        // 0 1 2 3 4 5
        // ( ) ) ( ( )
        let start_matches = vec![(0, 0), (3, 3), (4, 4)];
        let end_matches = vec![(1, 1), (2, 2), (5, 5)];
        let start_matches = start_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let end_matches = end_matches
            .into_iter()
            .map(|(start, end)| (start..end))
            .map(|x| RegexMatch {
                match_range: x.clone(),
                capture_range: x,
            })
            .collect::<Vec<_>>();
        let result = find_pairs("())(()", &start_matches, &end_matches);
        assert!(result.is_err());
    }

    #[test]
    fn test_mean_ok() {
        let data = vec![1., 2., 3.];
        let result = mean(&data);
        assert_eq!(result, Some(2.));

        let data = vec![1., 2.];
        let result = mean(&data);
        assert_eq!(result, Some(1.5));

        let data = vec![];
        let result = mean(&data);
        assert_eq!(result, None);
    }
}

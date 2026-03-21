const MAX_KEYWORD_DIFFERENCE_SCORE: f64 = 7.0;

fn extract_numbers(s: &str) -> Vec<&str> {
    let mut numbers = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s_idx) = start.take() {
            numbers.push(&s[s_idx..i]);
        }
    }
    if let Some(s_idx) = start {
        numbers.push(&s[s_idx..]);
    }
    numbers
}

pub fn weighted_levenshtein(a: &str, b: &str) -> Option<f64> {
    if extract_numbers(a) != extract_numbers(b) {
        return None;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (m, n) = (a_chars.len(), b_chars.len());

    let mut dp = vec![vec![0.0; n + 1]; m + 1];
    for i in 1..=m {
        dp[i][0] = dp[i - 1][0] + del_cost(a_chars[i - 1]);
    }
    for j in 1..=n {
        dp[0][j] = dp[0][j - 1] + ins_cost(b_chars[j - 1]);
    }

    for i in 1..=m {
        let mut min_in_row = f64::INFINITY;
        let mut any_valid = false;
        for j in 1..=n {
            let sub = if a_chars[i - 1] == b_chars[j - 1] {
                0.0
            } else {
                sub_cost(a_chars[i - 1], b_chars[j - 1])
            };
            dp[i][j] = min3(
                dp[i - 1][j] + del_cost(a_chars[i - 1]),
                dp[i][j - 1] + ins_cost(b_chars[j - 1]),
                dp[i - 1][j - 1] + sub,
            );
            if dp[i][j] <= MAX_KEYWORD_DIFFERENCE_SCORE {
                any_valid = true;
            }
            if dp[i][j] < min_in_row {
                min_in_row = dp[i][j];
            }
        }
        // Only exit early if no cell in this row is valid AND the minimum
        // value in the row exceeds the threshold
        if !any_valid && min_in_row > MAX_KEYWORD_DIFFERENCE_SCORE {
            return None;
        }
    }

    let result = dp[m][n];
    if result > MAX_KEYWORD_DIFFERENCE_SCORE {
        None
    } else {
        Some(result)
    }
}

fn min3(a: f64, b: f64, c: f64) -> f64 {
    a.min(b).min(c)
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ':' | ';' | '-' | '_' | '(' | ')' | '[' | ']' | ' '
    )
}

fn del_cost(c: char) -> f64 {
    if c.is_ascii_digit() {
        2.0
    } else if is_punct(c) {
        0.5
    } else {
        1.0
    }
}

fn ins_cost(c: char) -> f64 {
    del_cost(c)
}

fn sub_cost(a: char, b: char) -> f64 {
    if a.is_ascii_digit() && b.is_ascii_digit() {
        2.0
    } else if is_punct(a) || is_punct(b) {
        0.5
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_have_zero_distance() {
        let a = "Ziemer Theorem 1.2.3";
        let b = "Ziemer Theorem 1.2.3";
        assert_eq!(weighted_levenshtein(a, b), Some(0.0));
    }

    #[test]
    fn numeric_difference_does_not_match() {
        let a = "Ziemer Theorem 1.2.3";
        let b = "Ziemer Theorem 1.2.2";
        assert!(weighted_levenshtein(a, b).is_none());
    }

    #[test]
    fn punctuation_difference_is_light() {
        let a = "Ziemer Theorem 1.2.3";
        let b = "ZiemerTheorem1.2.3";
        let dist = weighted_levenshtein(a, b).unwrap();
        assert!(dist < 2.0, "got {dist}");
    }

    #[test]
    fn completely_different_returns_none() {
        let a = "Ziemer Theorem 1.2.3";
        let b = "Smith Lemma 5.1";
        assert!(weighted_levenshtein(a, b).is_none());
    }

    #[test]
    fn long_but_close_is_accepted() {
        let a = "Ziemer Theorem 12.3.4";
        let b = "Ziemer  Theorem 12.3.4";
        assert!(weighted_levenshtein(a, b).is_some());
    }

    #[test]
    fn same_numbers_different_chapter_label_does_not_match() {
        let a = "Ziemer Chapter 2 Theorem 5.1";
        let b = "Ziemer Chapter 2 Theorem 5.2";
        assert!(weighted_levenshtein(a, b).is_none());
    }

    #[test]
    fn same_reference_matches_despite_spacing() {
        let a = "Ziemer 5.1";
        let b = "Ziemer  5.1";
        assert!(weighted_levenshtein(a, b).is_some());
    }

    #[test]
    fn early_exit_triggered_for_large_difference() {
        // Should exceed MAX_KEYWORD_DIFFERENCE_SCORE quickly
        let a = "Ziemer Theorem 1.2.3";
        let b = "Completely unrelated long phrase with other numbers 9999";
        assert!(weighted_levenshtein(a, b).is_none());
    }
}

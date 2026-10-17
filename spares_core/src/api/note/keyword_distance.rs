//! Matching a searched keyword (the text of a note link) against note keywords.
//!
//! Keywords are compared word by word rather than character by character, so that a match
//! cannot swap out a whole word, such as the author in `Ziemer 6.20` against `Joshi 6.20`, or
//! the subject in `Spectral Theorem` against `Spectral Norm`. Each word of one keyword must line
//! up, in order, with a word of the other that is the same up to case, accents, possessives,
//! plurals, a small typo or a different ending of the same stem. Only short filler words (`of`,
//! `the`, ...) may be left over on either side, and words containing digits must be identical.
//!
//! Scores are distances: `0.0` for keywords that are equal ignoring case, and larger for looser
//! matches. A searched keyword links to the matching keyword with the lowest score.
//!
//! # Algorithm
//!
//! 1. Both keywords are [normalized](NormalizedKeyword::new): accents are removed, letters are
//!    lowercased and the text is split into words at anything other than a letter or digit, so
//!    that dashes, spaces and other punctuation all separate words alike. Section numbers like
//!    `1.2.3` stay one word, a trailing `'s` or `'` is dropped and plurals are reduced to a
//!    common stem.
//! 2. Keywords equal ignoring case score `0.0`. Keywords whose runs of digits differ never match.
//! 3. The words are aligned in order, with a dynamic program like Levenshtein distance over
//!    words instead of characters. Two words may only be aligned if they are equal (`0.0`),
//!    differ by a plural (`0.5`), differ by a typo that keeps the first letter, of one edit in a
//!    word of 6 to 9 letters or up to two in a longer one (`1.0` per edit), or share a stem of
//!    at least 6 letters and three fifths of the longer word (`2.0`). Words with digits must be
//!    equal, and words of 5 letters or fewer, which are often names or acronyms, may not have
//!    typos. Only stop words may be left unaligned (`0.5` each). The score is `0.5` plus the
//!    cost of the cheapest alignment.
//! 4. Otherwise, keywords whose words run together are equal still match, with a score of `1.0`.
//!
//! Matching costs a pass over the words of each keyword, which keeps it fast enough to compare
//! every note link against every keyword.
//!
//! # Examples
//!
//! | Searched keyword                  | Keyword                         | Score | Why                |
//! |-----------------------------------|---------------------------------|-------|--------------------|
//! | `Gauss's Lemma`                   | `gauss's lemma`                 | 0.0   | Case only          |
//! | `Bolzano–Weierstrass Theorem`     | `Bolzano-Weierstrass Theorem`   | 0.5   | Punctuation        |
//! | `Kolmogorov's Continuity Theorem` | `Kolmogorov Continuity Theorem` | 0.5   | Possessive         |
//! | `distributions`                   | `Distribution`                  | 1.0   | Plural             |
//! | `the class equation`              | `Class Equation`                | 1.0   | Stop word          |
//! | `pushforward measure`             | `push-forward measure`          | 1.0   | Words run together |
//! | `Kolmogorov Continuty Theorem`    | `Kolmogorov Continuity Theorem` | 1.5   | Typo               |
//! | `isometrically isomorphic`        | `Isometric isomorphism`         | 4.5   | Two shared stems   |
//! | `Ziemer 6.20`                     | `Joshi 6.20`                    | none  | Different word     |
//! | `Spectral Theorem`                | `Spectral Norm`                 | none  | Different word     |
//! | `variation measure`               | `Total variation measure`       | none  | Extra word         |
//! | `FTC`                             | `FIP`                           | none  | Short word differs |
//! | `Martingale`                      | `Submartingale`                 | none  | Different word     |
//! | `Ziemer 5.1`                      | `Ziemer 5.2`                    | none  | Number differs     |
//! | `Chapter 2 Theorem 5`             | `Chapter 5 Theorem 2`           | none  | Numbers reordered  |

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Words that may be present in one keyword but not the other.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "for", "in", "of", "on", "the", "to", "with",
];

/// Distance of keywords whose words all match exactly but whose text differs, for example in
/// punctuation, accents or possessives.
const BASE_COST: f64 = 0.5;
/// Cost of a word that only differs from its counterpart by a plural.
const PLURAL_COST: f64 = 0.5;
/// Cost of each edit of a word with a typo.
const TYPO_COST: f64 = 1.0;
/// Cost of a word that shares a long stem with its counterpart, like `isometric` and
/// `isometrically`.
const STEM_COST: f64 = 2.0;
/// Cost of a stop word present in only one keyword.
const STOP_WORD_COST: f64 = 0.5;
/// Cost of keywords that only match once their words are run together, like `pushforward` and
/// `push-forward`.
const JOINED_COST: f64 = 1.0;

#[derive(Debug)]
struct Word {
    /// Lowercase, without accents, apostrophes or possessive.
    text: String,
    /// `text` without a plural ending.
    stem: String,
    stem_chars: Vec<char>,
    has_digit: bool,
    is_stop: bool,
}

/// A keyword preprocessed for [`keyword_distance`], so that matching one searched keyword
/// against many keywords normalizes each of them only once.
#[derive(Debug)]
pub struct NormalizedKeyword {
    lowercase: String,
    words: Vec<Word>,
    /// Stems of all words run together.
    joined: String,
    /// Runs of digits in the keyword, in order.
    numbers: Vec<String>,
}

impl NormalizedKeyword {
    pub fn new(keyword: &str) -> Self {
        let folded: String = keyword
            .nfd()
            .filter(|c| !is_combining_mark(*c))
            .flat_map(char::to_lowercase)
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' | '\u{02BC}' | '`' => '\'',
                c => c,
            })
            .collect();
        let chars: Vec<char> = folded.chars().collect();

        let mut words = Vec::new();
        let mut current = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let prev_is_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next = chars.get(i + 1);
            if c.is_alphanumeric()
                // Keep section numbers like `1.2.3` as a single word.
                || (c == '.' && prev_is_digit && next.is_some_and(char::is_ascii_digit))
                || (c == '\'' && !current.is_empty())
            {
                current.push(c);
            } else if !current.is_empty() {
                words.push(Word::new(&std::mem::take(&mut current)));
            }
        }
        if !current.is_empty() {
            words.push(Word::new(&current));
        }

        let joined = words.iter().map(|w| w.stem.as_str()).collect();
        Self {
            lowercase: keyword.to_lowercase(),
            words,
            joined,
            numbers: extract_numbers(&folded),
        }
    }

    fn has_content_word(&self) -> bool {
        self.words.iter().any(|w| !w.is_stop)
    }
}

impl Word {
    fn new(raw: &str) -> Self {
        let without_possessive = raw
            .strip_suffix("'s")
            .or_else(|| raw.strip_suffix('\''))
            .unwrap_or(raw);
        let text: String = without_possessive.chars().filter(|&c| c != '\'').collect();
        let has_digit = text.chars().any(|c| c.is_ascii_digit());
        let stem = if has_digit {
            text.clone()
        } else {
            singular(&text)
        };
        let is_stop = STOP_WORDS.contains(&text.as_str());
        Self {
            text,
            stem_chars: stem.chars().collect(),
            stem,
            has_digit,
            is_stop,
        }
    }
}

/// Strips a plural ending. This is only meant to agree with itself on both keywords, not to
/// always produce real words.
fn singular(word: &str) -> String {
    let len = word.chars().count();
    if len > 4
        && let Some(stem) = word.strip_suffix("ies")
    {
        return format!("{stem}y");
    }
    if len > 3 && word.ends_with('s') && !["ss", "us", "is"].iter().any(|end| word.ends_with(end)) {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

fn extract_numbers(s: &str) -> Vec<String> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            numbers.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        numbers.push(current);
    }
    numbers
}

/// The distance between two keywords, or `None` if they do not match.
pub fn keyword_distance(a: &NormalizedKeyword, b: &NormalizedKeyword) -> Option<f64> {
    if a.lowercase == b.lowercase {
        return Some(0.0);
    }
    if a.numbers != b.numbers {
        return None;
    }
    let joined = (!a.joined.is_empty() && a.joined == b.joined).then_some(JOINED_COST);
    let aligned = if a.has_content_word() && b.has_content_word() {
        align_words(&a.words, &b.words).map(|cost| BASE_COST + cost)
    } else {
        None
    };
    match (aligned, joined) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

/// The cheapest in-order alignment of the words of `a` with the words of `b`, where only stop
/// words may be left unaligned.
fn align_words(a: &[Word], b: &[Word]) -> Option<f64> {
    let gap = |w: &Word| {
        if w.is_stop {
            STOP_WORD_COST
        } else {
            f64::INFINITY
        }
    };
    let mut prev = vec![0.0; b.len() + 1];
    for j in 1..=b.len() {
        prev[j] = prev[j - 1] + gap(&b[j - 1]);
    }
    let mut row = vec![0.0; b.len() + 1];
    for wa in a {
        row[0] = prev[0] + gap(wa);
        for (j, wb) in b.iter().enumerate() {
            let substitute = word_distance(wa, wb).map_or(f64::INFINITY, |d| prev[j] + d);
            row[j + 1] = substitute.min(prev[j + 1] + gap(wa)).min(row[j] + gap(wb));
        }
        if row.iter().all(|c| c.is_infinite()) {
            return None;
        }
        std::mem::swap(&mut prev, &mut row);
    }
    let cost = prev[b.len()];
    cost.is_finite().then_some(cost)
}

fn word_distance(a: &Word, b: &Word) -> Option<f64> {
    if a.text == b.text {
        return Some(0.0);
    }
    if a.has_digit || b.has_digit {
        return None;
    }
    if a.stem == b.stem {
        return Some(PLURAL_COST);
    }
    let (a, b) = (&a.stem_chars, &b.stem_chars);
    let max_len = a.len().max(b.len());

    // Short words like names and acronyms (`Joshi`, `FTC`) must match exactly.
    let allowed_typos = match max_len {
        0..=5 => 0,
        6..=9 => 1,
        _ => 2,
    };
    if allowed_typos > 0
        && a[0] == b[0]
        && let Some(edits) = edit_distance(a, b, allowed_typos)
    {
        return Some(TYPO_COST * f64::from(u8::try_from(edits).ok()?));
    }

    let common_prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    (common_prefix >= 6 && common_prefix * 5 >= max_len * 3).then_some(STEM_COST)
}

/// The optimal string alignment distance (Levenshtein with adjacent transpositions), or `None`
/// if it exceeds `max`.
fn edit_distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev_prev: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            let sub = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + sub);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(prev_prev[j - 2] + 1);
            }
        }
        if row.iter().all(|&d| d > max) {
            return None;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut row);
    }
    Some(prev[b.len()]).filter(|&d| d <= max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: &str, b: &str) -> Option<f64> {
        keyword_distance(&NormalizedKeyword::new(a), &NormalizedKeyword::new(b))
    }

    #[test]
    fn identical_strings_have_zero_distance() {
        assert_eq!(
            distance("Ziemer Theorem 1.2.3", "Ziemer Theorem 1.2.3"),
            Some(0.0)
        );
        assert_eq!(distance("Gauss's Lemma", "gauss's lemma"), Some(0.0));
    }

    #[test]
    fn numeric_difference_does_not_match() {
        assert_eq!(
            distance("Ziemer Theorem 1.2.3", "Ziemer Theorem 1.2.2"),
            None
        );
        assert_eq!(
            distance(
                "Ziemer Chapter 2 Theorem 5.1",
                "Ziemer Chapter 2 Theorem 5.2"
            ),
            None
        );
        assert_eq!(distance("Ziemer 1 2", "Ziemer 12"), None);
        assert_eq!(distance("Chapter 2 Theorem 5", "Chapter 5 Theorem 2"), None);
    }

    #[test]
    fn different_author_does_not_match() {
        assert_eq!(distance("Ziemer 6.20", "Joshi 6.20"), None);
        assert_eq!(distance("Royden 3.24", "Joshi 3.24"), None);
        assert_eq!(distance("Ziemer 4.7", "Crack 4.7"), None);
    }

    #[test]
    fn different_word_does_not_match() {
        for (a, b) in [
            ("Spectral Theorem", "Spectral Norm"),
            ("Sylvester's Criterion", "Eisenstein's Criterion"),
            ("Rational Canonical Form", "Jordan Canonical Form"),
            ("Besicovitch's Theorem", "Ascoli's Theorem"),
            ("Itô integral", "Poisson integral"),
            ("Martingale Strategy", "Martingale Property"),
            ("Banach Fixed-Point Theorem", "Brouwer fixed-point theorem"),
            ("variation measure", "Radon measure"),
            ("FTC", "FIP"),
            ("Martingale", "Submartingale"),
            ("Martingale", "Sub-martingale"),
            ("Lemma", "The Lemma Theorem"),
        ] {
            assert_eq!(distance(a, b), None, "{a} matched {b}");
        }
    }

    #[test]
    fn stop_words_alone_do_not_match() {
        assert_eq!(distance("The", "A"), None);
    }

    #[test]
    fn spelling_variants_match() {
        for (a, b) in [
            (
                "Kolmogorov's Continuity Theorem",
                "Kolmogorov Continuity Theorem",
            ),
            ("Gauss' Lemma", "Gauss's Lemma"),
            ("Gauss’s Lemma", "Gauss's Lemma"),
            ("distributions", "Distribution"),
            ("keywords 1", "keyword 1"),
            ("Blaschke factor", "Blaschke factors"),
            ("Bolzano–Weierstrass Theorem", "Bolzano-Weierstrass Theorem"),
            ("Picard-Lindelöf", "Picard–Lindelof"),
            ("pushforward measure", "push-forward measure"),
            ("isometrically isomorphic", "Isometric isomorphism"),
            (
                "Fundamental Theorem Calculus",
                "Fundamental Theorem of Calculus",
            ),
            (
                "Kolmogorov Continuty Theorem",
                "Kolmogorov Continuity Theorem",
            ),
            ("Zeimer 5.1", "Ziemer 5.1"),
            ("Ziemer 5.1", "Ziemer  5.1"),
            ("Ziemer Theorem 1.2.3", "ZiemerTheorem1.2.3"),
        ] {
            assert!(
                distance(a, b).is_some_and(|d| d > 0.0),
                "{a} did not match {b}"
            );
        }
    }

    #[test]
    fn closer_variants_score_lower() {
        let possessive = distance(
            "Kolmogorov's Continuity Theorem",
            "Kolmogorov Continuity Theorem",
        );
        let typo = distance(
            "Kolmogorov Continuty Theorem",
            "Kolmogorov Continuity Theorem",
        );
        let stem = distance("isometrically isomorphic", "Isometric isomorphism");
        assert!(possessive < typo, "{possessive:?} {typo:?}");
        assert!(typo < stem, "{typo:?} {stem:?}");
    }

    /// Keeps the examples in the module documentation true.
    #[test]
    fn documented_examples() {
        let examples = [
            ("Gauss's Lemma", "gauss's lemma", Some(0.0)),
            (
                "Bolzano–Weierstrass Theorem",
                "Bolzano-Weierstrass Theorem",
                Some(0.5),
            ),
            (
                "Kolmogorov's Continuity Theorem",
                "Kolmogorov Continuity Theorem",
                Some(0.5),
            ),
            ("distributions", "Distribution", Some(1.0)),
            ("the class equation", "Class Equation", Some(1.0)),
            ("pushforward measure", "push-forward measure", Some(1.0)),
            (
                "Kolmogorov Continuty Theorem",
                "Kolmogorov Continuity Theorem",
                Some(1.5),
            ),
            (
                "isometrically isomorphic",
                "Isometric isomorphism",
                Some(4.5),
            ),
            ("Ziemer 6.20", "Joshi 6.20", None),
            ("Spectral Theorem", "Spectral Norm", None),
            ("variation measure", "Total variation measure", None),
            ("FTC", "FIP", None),
            ("Martingale", "Submartingale", None),
            ("Ziemer 5.1", "Ziemer 5.2", None),
            ("Chapter 2 Theorem 5", "Chapter 5 Theorem 2", None),
        ];
        for (a, b, expected) in examples {
            assert_eq!(distance(a, b), expected, "{a} against {b}");
        }
    }
}

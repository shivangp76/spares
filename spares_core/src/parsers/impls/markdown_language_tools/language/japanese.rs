//! Japanese: kana readings, which text to speech reads, and deinflection.
//!
//! Deinflection works as Yomitan's does (`japanese-transforms.js`, GPL-3.0, ported in a smaller
//! form): each rule replaces an inflected ending with another (`食べた` → `食べる`), if the text is
//! the kind of word the rule inflects from. Rules are applied again to what they give, so
//! `食べさせられなかった` goes back through `食べさせられない`, `食べさせられる` and `食べさせる` to
//! `食べる`. Each result says what kind of word it must be (`v1`, `v5`, …), which a dictionary
//! entry is checked against.

use std::collections::HashSet;
use std::sync::LazyLock;

use super::CjkFont;
use super::Deinflection;
use super::Language;

pub static JAPANESE: Language = Language {
    code: "ja",
    name: "Japanese",
    written_with_spaces: false,
    speak_reading: true,
    // Harano Aji, in TeX Live, which is also luatexja's default
    cjk_font: Some(CjkFont {
        regular: "HaranoAjiMincho-Regular.otf",
        bold: "HaranoAjiGothic-Medium.otf",
    }),
    deinflect,
};

/// Kinds of words, as bits. A rule inflects from (`rules_in`) and to (`rules_out`) a set of them.
const V1: u8 = 1; // ichidan verb (食べる)
const V5: u8 = 1 << 1; // godan verb (書く)
const VS: u8 = 1 << 2; // suru verb (する, 勉強する)
const VK: u8 = 1 << 3; // kuru verb (来る)
const ADJ_I: u8 = 1 << 4; // i-adjective (高い), and what inflects like one (食べない, 食べたい)
const TE: u8 = 1 << 5; // a te-form (食べて), which ている and てしまう attach to
const MASU: u8 = 1 << 6; // a polite form (食べます), which inflects on

/// The rule identifiers of Yomitan dictionaries for each kind.
const KIND_NAMES: [(u8, &str); 5] = [
    (V1, "v1"),
    (V5, "v5"),
    (VS, "vs"),
    (VK, "vk"),
    (ADJ_I, "adj-i"),
];

#[derive(Debug)]
struct Rule {
    reason: &'static str,
    from: String,
    to: String,
    /// The kinds of word the inflected text is. A text first looked up is any kind.
    rules_in: u8,
    rules_out: u8,
}

/// The rows of godan verbs: the dictionary ending, and its i, a, e and o stems, te-form and past.
const GODAN: [[&str; 7]; 9] = [
    ["う", "い", "わ", "え", "お", "って", "った"],
    ["く", "き", "か", "け", "こ", "いて", "いた"],
    ["ぐ", "ぎ", "が", "げ", "ご", "いで", "いだ"],
    ["す", "し", "さ", "せ", "そ", "して", "した"],
    ["つ", "ち", "た", "て", "と", "って", "った"],
    ["ぬ", "に", "な", "ね", "の", "んで", "んだ"],
    ["ぶ", "び", "ば", "べ", "ぼ", "んで", "んだ"],
    ["む", "み", "ま", "め", "も", "んで", "んだ"],
    ["る", "り", "ら", "れ", "ろ", "って", "った"],
];

/// The endings of the polite forms after a verb's i-stem.
const MASU_FORMS: [(&str, &str); 6] = [
    ("ます", "polite"),
    ("ません", "polite negative"),
    ("ました", "polite past"),
    ("ませんでした", "polite past negative"),
    ("ましょう", "polite volitional"),
    ("まして", "polite te-form"),
];

/// Stems a verb kind's forms are built from: what replaces the dictionary ending before each
/// form's ending.
struct VerbStems<'a> {
    dictionary: &'a str,
    /// For ます, たい and the masu stem
    i: &'a str,
    /// For ない, ず, れる and せる
    a: &'a str,
    /// For ば and the potential
    e: &'a str,
    /// For the volitional
    volitional: &'a str,
    imperative: &'a [&'a str],
    te: &'a str,
    past: &'a str,
    passive: &'a str,
    causative: &'a str,
    potential: &'a [&'a str],
    kind: u8,
}

fn verb_rules(rules: &mut Vec<Rule>, stems: &VerbStems<'_>) {
    let mut add = |reason, from: String, rules_in| {
        rules.push(Rule {
            reason,
            from,
            to: stems.dictionary.to_string(),
            rules_in,
            rules_out: stems.kind,
        });
    };
    add("negative", format!("{}ない", stems.a), ADJ_I);
    add("negative", format!("{}ず", stems.a), 0);
    add("negative", format!("{}ぬ", stems.a), 0);
    add("negative te-form", format!("{}ないで", stems.a), 0);
    add("negative conditional", format!("{}なければ", stems.a), 0);
    if !stems.passive.is_empty() {
        add("passive", format!("{}{}", stems.a, stems.passive), V1);
        add("causative", format!("{}{}", stems.a, stems.causative), V1);
    }
    add("past", stems.past.to_string(), 0);
    add("te-form", stems.te.to_string(), TE);
    add("tara", format!("{}ら", stems.past), 0);
    add("tari", format!("{}り", stems.past), 0);
    add("conditional", format!("{}ば", stems.e), 0);
    add("volitional", stems.volitional.to_string(), 0);
    for imperative in stems.imperative {
        add("imperative", (*imperative).to_string(), 0);
    }
    for potential in stems.potential {
        add("potential", (*potential).to_string(), V1);
    }
    add("desire", format!("{}たい", stems.i), ADJ_I);
    add("polite imperative", format!("{}なさい", stems.i), 0);
    add("masu stem", stems.i.to_string(), 0);
    add("too much", format!("{}すぎる", stems.i), V1);
    add("while", format!("{}ながら", stems.i), 0);
    for (ending, reason) in MASU_FORMS {
        add(reason, format!("{}{ending}", stems.i), MASU);
    }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let mut rules = Vec::new();
    for [dictionary, i, a, e, o, te, past] in GODAN {
        let volitional = format!("{o}う");
        let potential = format!("{e}る");
        let causative_short = format!("{a}す");
        verb_rules(
            &mut rules,
            &VerbStems {
                dictionary,
                i,
                a,
                e,
                volitional: &volitional,
                imperative: &[e],
                te,
                past,
                passive: "れる",
                causative: "せる",
                potential: &[&potential],
                kind: V5,
            },
        );
        // 書かす, the short causative
        rules.push(Rule {
            reason: "causative",
            from: causative_short,
            to: dictionary.to_string(),
            rules_in: V5,
            rules_out: V5,
        });
    }
    // 行く's te-form and past are 行って and 行った
    for prefix in ["行", "い", "逝", "往"] {
        for (ending, reason, rules_in) in [
            ("って", "te-form", TE),
            ("った", "past", 0),
            ("ったら", "tara", 0),
            ("ったり", "tari", 0),
        ] {
            rules.push(Rule {
                reason,
                from: format!("{prefix}{ending}"),
                to: format!("{prefix}く"),
                rules_in,
                rules_out: V5,
            });
        }
    }
    verb_rules(
        &mut rules,
        &VerbStems {
            dictionary: "る",
            i: "",
            a: "",
            e: "れ",
            volitional: "よう",
            imperative: &["ろ", "よ"],
            te: "て",
            past: "た",
            passive: "られる",
            causative: "させる",
            // And ら抜き言葉, 食べれる
            potential: &["られる", "れる"],
            kind: V1,
        },
    );
    for prefix in ["", "為"] {
        let stem = |s: &str| format!("{prefix}{s}");
        let (i, e, te, past) = (stem("し"), stem("すれ"), stem("して"), stem("した"));
        let (volitional, imperative) = (stem("しよう"), [stem("しろ"), stem("せよ")]);
        let imperative = imperative.iter().map(String::as_str).collect::<Vec<_>>();
        let potential = [stem("できる")];
        let potential = potential.iter().map(String::as_str).collect::<Vec<_>>();
        verb_rules(
            &mut rules,
            &VerbStems {
                dictionary: &stem("する"),
                i: &i,
                a: &i,
                e: &e,
                volitional: &volitional,
                imperative: &imperative,
                te: &te,
                past: &past,
                passive: "",
                causative: "",
                potential: &potential,
                kind: VS,
            },
        );
        // される and させる, rather than しれる and しせる
        for (from, reason) in [("される", "passive"), ("させる", "causative")] {
            rules.push(Rule {
                reason,
                from: stem(from),
                to: stem("する"),
                rules_in: V1,
                rules_out: VS,
            });
        }
    }
    for (prefix_i, prefix_a, prefix_u) in [("き", "こ", "く"), ("来", "来", "来")] {
        let i = prefix_i.to_string();
        let a = prefix_a.to_string();
        let (e, te, past) = (
            format!("{prefix_u}れ"),
            format!("{prefix_i}て"),
            format!("{prefix_i}た"),
        );
        let volitional = format!("{prefix_a}よう");
        let imperative = [format!("{prefix_a}い")];
        let imperative = imperative.iter().map(String::as_str).collect::<Vec<_>>();
        let potential = [format!("{prefix_a}られる"), format!("{prefix_a}れる")];
        let potential = potential.iter().map(String::as_str).collect::<Vec<_>>();
        verb_rules(
            &mut rules,
            &VerbStems {
                dictionary: &format!("{prefix_u}る"),
                i: &i,
                a: &a,
                e: &e,
                volitional: &volitional,
                imperative: &imperative,
                te: &te,
                past: &past,
                passive: "られる",
                causative: "させる",
                potential: &potential,
                kind: VK,
            },
        );
    }
    // i-adjectives
    for (from, reason, rules_in) in [
        ("く", "adverbial", 0),
        ("くない", "negative", ADJ_I),
        ("かった", "past", 0),
        ("くて", "te-form", 0),
        ("ければ", "conditional", 0),
        ("かったら", "tara", 0),
        ("かろう", "volitional", 0),
        ("さ", "noun", 0),
        ("そう", "seemingness", 0),
        ("すぎる", "too much", V1),
        ("くなる", "become", V5),
    ] {
        rules.push(Rule {
            reason,
            from: from.to_string(),
            to: "い".to_string(),
            rules_in,
            rules_out: ADJ_I,
        });
    }
    // Forms that a te-form takes: ている, てしまう and their contractions
    for (from, to, reason, rules_in) in [
        ("ている", "て", "progressive", V1),
        ("でいる", "で", "progressive", V1),
        ("てる", "て", "progressive", V1),
        ("でる", "で", "progressive", V1),
        ("ておる", "て", "progressive", V5),
        ("てある", "て", "resultative", V5),
        ("ておく", "て", "in advance", V5),
        ("でおく", "で", "in advance", V5),
        ("とく", "て", "in advance", V5),
        ("どく", "で", "in advance", V5),
        ("てしまう", "て", "completion", V5),
        ("でしまう", "で", "completion", V5),
        ("ちゃう", "て", "completion", V5),
        ("じゃう", "で", "completion", V5),
        ("てください", "て", "request", 0),
        ("でください", "で", "request", 0),
        ("てくる", "て", "coming", VK),
        ("ていく", "て", "going", V5),
        ("てみる", "て", "trying", V1),
    ] {
        rules.push(Rule {
            reason,
            from: from.to_string(),
            to: to.to_string(),
            rules_in,
            rules_out: TE,
        });
    }
    rules
});

/// The kind names of `kinds`, for a dictionary entry's rules.
fn kind_names(kinds: u8) -> Vec<&'static str> {
    KIND_NAMES
        .iter()
        .filter(|(kind, _)| kinds & kind != 0)
        .map(|(_, name)| *name)
        .collect()
}

/// The dictionary forms `text` may be inflected from (see the [module docs](self)).
pub fn deinflect(text: &str) -> Vec<Deinflection> {
    struct Candidate {
        term: String,
        rules: u8,
        reasons: Vec<&'static str>,
    }
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![Candidate {
        term: text.to_string(),
        rules: 0,
        reasons: Vec::new(),
    }];
    while let Some(candidate) = queue.pop() {
        for rule in RULES.iter() {
            if candidate.rules != 0 && candidate.rules & rule.rules_in == 0 {
                continue;
            }
            let Some(stem) = candidate.term.strip_suffix(rule.from.as_str()) else {
                continue;
            };
            if stem.is_empty() && rule.to.chars().count() < 2 {
                continue;
            }
            let term = format!("{stem}{}", rule.to);
            if !seen.insert((term.clone(), rule.rules_out)) {
                continue;
            }
            let mut reasons = vec![rule.reason];
            reasons.extend(&candidate.reasons);
            // Results that are only a step towards another form are not words
            let rules = rule.rules_out & !(TE | MASU);
            if rules != 0 {
                results.push(Deinflection {
                    term: term.clone(),
                    rules: kind_names(rules),
                    reasons: reasons.clone(),
                });
            }
            queue.push(Candidate {
                term,
                rules: rule.rules_out,
                reasons,
            });
        }
        // Deep chains only come from texts that aren't words
        if results.len() > 512 {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `text` deinflects to `term` as the kind `rule`.
    fn deinflects_to(text: &str, term: &str, rule: &str) -> bool {
        deinflect(text)
            .iter()
            .any(|d| d.term == term && d.rules.contains(&rule))
    }

    #[test]
    fn test_verbs() {
        for (text, term, rule) in [
            ("食べた", "食べる", "v1"),
            ("食べない", "食べる", "v1"),
            ("食べなかった", "食べる", "v1"),
            ("食べます", "食べる", "v1"),
            ("食べませんでした", "食べる", "v1"),
            ("食べている", "食べる", "v1"),
            ("食べてる", "食べる", "v1"),
            ("食べていた", "食べる", "v1"),
            ("食べさせられなかった", "食べる", "v1"),
            ("食べたい", "食べる", "v1"),
            ("食べれば", "食べる", "v1"),
            ("書いた", "書く", "v5"),
            ("書かない", "書く", "v5"),
            ("書きます", "書く", "v5"),
            ("書ける", "書く", "v5"),
            ("書いてしまった", "書く", "v5"),
            ("書いちゃった", "書く", "v5"),
            ("泳いで", "泳ぐ", "v5"),
            ("読んだ", "読む", "v5"),
            ("話して", "話す", "v5"),
            ("待った", "待つ", "v5"),
            ("買わなかった", "買う", "v5"),
            ("行った", "行く", "v5"),
            ("帰ろう", "帰る", "v5"),
            ("した", "する", "vs"),
            ("勉強しています", "勉強する", "vs"),
            ("勉強させられた", "勉強する", "vs"),
            ("来た", "来る", "vk"),
            ("こない", "くる", "vk"),
            ("きました", "くる", "vk"),
        ] {
            assert!(
                deinflects_to(text, term, rule),
                "{text} → {term} ({rule}): {:?}",
                deinflect(text)
                    .iter()
                    .map(|d| (&d.term, &d.rules))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_adjectives() {
        for (text, term) in [
            ("高かった", "高い"),
            ("高くない", "高い"),
            ("高くなかった", "高い"),
            ("高くて", "高い"),
            ("高ければ", "高い"),
            ("高さ", "高い"),
        ] {
            assert!(deinflects_to(text, term, "adj-i"), "{text} → {term}");
        }
    }

    #[test]
    fn test_reasons_and_kinds() {
        let results = deinflect("食べなかった");
        let result = results.iter().find(|d| d.term == "食べる").unwrap();
        assert_eq!(result.reasons, ["negative", "past"]);
        // 食べ isn't deinflected to a te-form or a polite form, which aren't words
        assert!(deinflect("食べて").iter().all(|d| !d.rules.is_empty()));
        // Words that aren't inflected give nothing that is wrong, only candidates
        assert!(!deinflects_to("猫", "猫", "v1"));
    }
}

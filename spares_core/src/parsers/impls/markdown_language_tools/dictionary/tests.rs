use std::io::Write as _;

use serde_json::Value;
use serde_json::json;
use sqlx::SqlitePool;

use super::import::import;
use super::import::parse_term_bank;
use super::lookup::LookupRequest;
use super::lookup::lookup;
use super::*;

/// A Yomitan dictionary zip with `index` and one term bank of `terms`.
pub(crate) fn dictionary_zip(index: &Value, terms: &Value) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("index.json", options).unwrap();
    zip.write_all(index.to_string().as_bytes()).unwrap();
    zip.start_file("term_bank_1.json", options).unwrap();
    zip.write_all(terms.to_string().as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

pub(crate) fn japanese_zip() -> Vec<u8> {
    dictionary_zip(
        &json!({"title": "Tiny JMdict", "revision": "1", "format": 3, "attribution": "CC BY-SA 4.0 EDRDG"}),
        &json!([
            ["食べる", "たべる", "v1 vt", "v1", 100, ["to eat"], 1, "P"],
            ["食べ物", "たべもの", "n", "", 90, ["food"], 2, ""],
            ["書く", "かく", "v5k", "v5", 80, [{"type": "structured-content", "content": [
                {"tag": "ul", "content": [{"tag": "li", "content": "to write"}, {"tag": "li", "content": ["to ", {"tag": "ruby", "content": ["描", {"tag": "rt", "content": "えが"}]}, "draw"]}]},
                {"tag": "img", "path": "a.png"}
            ]}], 3, ""],
            ["高い", "たかい", "adj-i", "adj-i", 70, ["high", "expensive"], 4, ""],
            ["食", "しょく", "n", "", 10, ["eating"], 5, ""],
            ["猫", "ねこ", "n", "", 60, [{"type": "image", "path": "cat.png"}], 6, ""]
        ]),
    )
}

async fn pool(name: &str) -> SqlitePool {
    let dir =
        std::env::temp_dir().join(format!("spares-dictionaries-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    open_at(dir.join("dictionaries.sqlite"), true)
        .await
        .unwrap()
        .unwrap()
}

async fn look_up(pool: &SqlitePool, text: &str, language: Option<&str>) -> lookup::LookupResponse {
    lookup(
        Some(pool),
        &LookupRequest {
            text: text.to_string(),
            language: language.map(str::to_string),
        },
    )
    .await
    .unwrap()
}

#[test]
fn test_structured_content() {
    let terms = parse_term_bank(
        &json!([["書く", "かく", "v5k", "v5", 80, [{"type": "structured-content", "content": [
            {"tag": "ul", "content": [{"tag": "li", "content": "to write"}, {"tag": "li", "content": ["to ", {"tag": "ruby", "content": ["描", {"tag": "rt", "content": "えが"}]}, "draw"]}]}
        ]}], 3, ""]])
        .to_string(),
        3,
    )
    .unwrap();
    assert_eq!(terms[0].glosses, ["• to write\n• to 描draw"]);
    // Format 1: the glossary's strings follow the score
    let terms = parse_term_bank(
        &json!([["猫", "ねこ", "n", "", 1, "cat", "kitty"]]).to_string(),
        1,
    )
    .unwrap();
    assert_eq!(terms[0].glosses, ["cat", "kitty"]);
}

#[tokio::test]
async fn test_import_and_lookup_japanese() {
    let pool = pool("ja").await;
    let info = import(&pool, &japanese_zip(), None).await.unwrap();
    // The entry with only an image is left out
    assert_eq!(info.term_count, 5);
    // From the terms' script
    assert_eq!(info.language.as_deref(), Some("ja"));
    assert_eq!(info.attribution, "CC BY-SA 4.0 EDRDG");

    // The longest prefix, deinflected
    let response = look_up(&pool, "食べなかったので", Some("ja")).await;
    assert_eq!(response.matched, 6);
    let first = &response.entries[0];
    assert_eq!(first.expression, "食べる");
    assert_eq!(first.source, "食べなかった");
    assert_eq!(first.inflections, ["negative", "past"]);
    assert_eq!(first.attribution, "CC BY-SA 4.0 EDRDG");
    // Shorter matches follow
    assert!(
        response
            .entries
            .iter()
            .any(|e| e.expression == "食" && e.matched == 1)
    );

    let response = look_up(&pool, "食べ物が好き", None).await;
    assert_eq!(response.entries[0].expression, "食べ物");
    assert_eq!(response.language.as_deref(), Some("ja"));
    // By reading
    assert_eq!(
        look_up(&pool, "かいた", Some("ja")).await.entries[0].expression,
        "書く"
    );
    // The kind of word is checked: 食べる isn't a godan verb, so 食べらない isn't it
    assert!(
        look_up(&pool, "高かった", Some("ja"))
            .await
            .entries
            .iter()
            .any(|e| e.expression == "高い")
    );
    assert!(
        look_up(&pool, "書かる", Some("ja"))
            .await
            .entries
            .iter()
            .all(|e| e.matched < 3)
    );
    // Nothing for text it doesn't have
    assert_eq!(look_up(&pool, "xyz", Some("ja")).await.entries.len(), 0);
}

#[tokio::test]
async fn test_languages_and_order() {
    let pool = pool("languages").await;
    import(&pool, &japanese_zip(), None).await.unwrap();
    let chinese = import(
        &pool,
        &dictionary_zip(
            &json!({"title": "Tiny CEDICT", "revision": "2", "sourceLanguage": "zh"}),
            &json!([
                ["食物", "shí wù", "", "", 0, ["food"], 1, ""],
                ["食", "shí", "", "", 0, ["to eat (zh)"], 2, ""]
            ]),
        ),
        None,
    )
    .await
    .unwrap();
    assert_eq!(chinese.language.as_deref(), Some("zh"));
    let english = import(
        &pool,
        &dictionary_zip(
            &json!({"title": "Tiny Wiktionary", "revision": "1"}),
            &json!([
                ["run", "", "v", "", 0, ["to move quickly"], 1, ""],
                [
                    "running",
                    "",
                    "v",
                    "",
                    0,
                    ["present participle of run"],
                    2,
                    ""
                ]
            ]),
        ),
        Some("en"),
    )
    .await
    .unwrap();

    // Each language's own dictionaries
    let zh = look_up(&pool, "食物很好", Some("zh")).await;
    assert_eq!(zh.entries[0].dictionary, "Tiny CEDICT");
    assert!(zh.entries.iter().all(|e| e.dictionary == "Tiny CEDICT"));
    let ja = look_up(&pool, "食", Some("ja")).await;
    assert!(ja.entries.iter().all(|e| e.dictionary == "Tiny JMdict"));
    // Han text without a language looks in every dictionary
    let any = look_up(&pool, "食", None).await;
    assert_eq!(any.entries.len(), 2);
    // A language written with spaces looks up the word, lower case
    let en = look_up(&pool, "Running fast", Some("en")).await;
    assert_eq!(en.matched, 7);
    assert_eq!(en.entries[0].expression, "running");

    // Disabled dictionaries aren't used, and the order is kept
    update(
        &pool,
        chinese.id,
        &UpdateDictionaryRequest {
            enabled: Some(false),
            language: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(look_up(&pool, "食物", Some("zh")).await.entries.len(), 0);
    let order = reorder(&pool, &[english.id]).await.unwrap();
    assert_eq!(
        order.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
        ["Tiny Wiktionary", "Tiny JMdict", "Tiny CEDICT"]
    );
    remove(&pool, english.id).await.unwrap();
    assert_eq!(look_up(&pool, "run", Some("en")).await.entries.len(), 0);
    assert!(matches!(
        remove(&pool, english.id).await,
        Err(DictionaryError::NotFound(_))
    ));
}

#[tokio::test]
async fn test_reimport() {
    let pool = pool("reimport").await;
    import(&pool, &japanese_zip(), None).await.unwrap();
    // The same revision is refused
    assert!(matches!(
        import(&pool, &japanese_zip(), None).await,
        Err(DictionaryError::Invalid(_))
    ));
    // Another revision replaces it
    let newer = dictionary_zip(
        &json!({"title": "Tiny JMdict", "revision": "2"}),
        &json!([["犬", "いぬ", "n", "", 0, ["dog"], 1, ""]]),
    );
    import(&pool, &newer, Some("ja")).await.unwrap();
    let dictionaries = list(&pool).await.unwrap();
    assert_eq!(dictionaries.len(), 1);
    assert_eq!(dictionaries[0].term_count, 1);
    // Not a dictionary
    assert!(matches!(
        import(&pool, b"not a zip", None).await,
        Err(DictionaryError::Invalid(_))
    ));
    assert!(matches!(
        import(&pool, &newer, Some("not a language")).await,
        Err(DictionaryError::Invalid(_))
    ));
}

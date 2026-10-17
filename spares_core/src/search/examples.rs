//! Example queries, each with what it finds, grouped by what they show. The web app's search
//! syntax popup and `spares note search --examples` list them, beside the fields (see
//! [`super::fields`]). The tests run each one, and check what some of them find.

use serde::Serialize;

/// A query, with what it finds.
#[derive(Debug, Clone, Serialize)]
pub struct QueryExample {
    /// The group it is listed under, like "Dates".
    pub section: &'static str,
    pub query: &'static str,
    /// What it finds. Text in backticks is code.
    pub about: &'static str,
}

const fn example(section: &'static str, query: &'static str, about: &'static str) -> QueryExample {
    QueryExample {
        section,
        query,
        about,
    }
}

#[rustfmt::skip]
const EXAMPLES: &[QueryExample] = &[
    example("Examples", "verb", "A word starting with `verb`, like `verbs`, in the note's text"),
    example("Examples", "\"past tense\"", "The text `past tense` anywhere"),
    example("Examples", "Paris or Rome", "Either word"),
    example("Examples", "tag=spanish -tag=\"spanish:verbs\"", "Tagged `spanish`, but not `spanish:verbs`"),
    example("Examples", "c.due<=today", "Cards due by the end of today"),
    example("Examples", "c.state=new created_at>=today-7d", "New cards of notes created this week"),
    example("Examples", "c.rated[>=today-7d]=1", "Cards forgotten this week"),
    example("Examples", "sort_by_asc=c.retrievability limit=20", "The 20 cards you are least likely to recall"),
    example("Examples", "linked_to_note=(tag=history)", "Notes linking to a note tagged `history`"),
    example("Examples", "(tag=a limit=5) or (tag=b limit=5)", "Up to 5 of each tag"),

    example("Text", "tag~history", "`~` finds text anywhere in a field: tags like `prehistory` and `history:rome`"),
    example("Text", "\"-ing\"", "Text with characters other than letters, digits, `.`, `_`, `-` and `+` goes in quotes"),
    example("Text", "#\"say \"hi\"\"#", "Text with quotes in it goes between `#\"` and `\"#`"),
    example("Text", "data~re:\"^The \"", "A regex, which is case sensitive"),
    example("Text", "\"or\"", "A word that is also a keyword, like `or`, `and` or `not`, goes in quotes"),

    example("Comparisons", "c.stability>=30", "Numbers compare with `=`, `!=`, `<`, `<=`, `>` and `>=`"),
    example("Comparisons", "c.state!=new", "Cards that are not new"),
    example("Comparisons", "-c.suspended", "`-` or `not` before a condition excludes what it matches"),
    example("Comparisons", "custom_data.flag", "Notes whose custom data has `flag`, other than `false` or `null`"),

    example("Dates", "created_at=2026-10", "A day, month or year means any time in it"),
    example("Dates", "c.due<=today+3d", "Due by the end of the day 3 days from now. Offsets are `m`, `h`, `d`, `w`, `mo` and `y`"),
    example("Dates", "updated_at>=now-12h", "Changed in the last 12 hours"),
    example("Dates", "created_at>=note[gir].created_at", "Created since note `gir` was"),

    example("Combining", "Paris Rome", "Both words: a space between conditions means `and`"),
    example("Combining", "a or b c", "`a`, or both `b` and `c`, since `and` goes before `or`"),
    example("Combining", "-(tag=a or tag=b)", "Parentheses group conditions: neither tag"),

    example("Sorting and limits", "sort_by_desc=c.difficulty,c.due", "Hardest first, and then by due date"),
    example("Sorting and limits", "Paris limit=5", "The 5 notes that match `Paris` best, since notes found by words come in order of relevance"),
    example("Sorting and limits", "((tag=a limit=5) or (tag=b limit=5)) limit=8", "Up to 5 of each tag, and 8 in all"),
];

/// Every example query, in the order they are listed.
pub fn examples() -> &'static [QueryExample] {
    EXAMPLES
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::Map;
    use sqlx::SqlitePool;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::model::NoteId;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::search::evaluator::Evaluator;
    use crate::search::fields;

    /// Notes with the text and tags of each pair, all with one card, whose cloze holds `zzz`.
    async fn create(pool: &SqlitePool, notes: &[(&str, &[&str])]) {
        let parser = create_parser_helper(pool, "markdown").await;
        let requests = notes
            .iter()
            .map(|(data, tags)| CreateNoteRequest {
                data: format!("{data} {{{{zzz}}}}"),
                keywords: Vec::new(),
                tags: tags.iter().map(ToString::to_string).collect(),
                is_suspended: false,
                custom_data: Map::new(),
            })
            .collect();
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests,
        };
        create_notes(pool, request, Utc::now(), &get_all_parsers(), false)
            .await
            .unwrap();
    }

    /// The text of the notes `query` finds, sorted.
    async fn found(pool: &SqlitePool, query: &str) -> Vec<String> {
        let notes = Evaluator::new(query).get_notes(pool).await;
        let mut found = notes
            .unwrap_or_else(|e| panic!("{query}: {e}"))
            .into_iter()
            .map(|(note, _)| note.data.split(" {{").next().unwrap().to_string())
            .collect::<Vec<_>>();
        found.sort();
        found
    }

    /// Every example, and every field's example, searches both notes and cards without an error.
    #[sqlx::test]
    async fn examples_run(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        create(&pool, &[("Paris", &["a"])]).await;
        // `note[gir]` must exist
        let gir = "gir".parse::<NoteId>().unwrap();
        sqlx::query(
            "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) SELECT ?, 'gir', 0, 0, '{}', parser_id FROM note",
        )
            .bind(gir)
            .execute(&pool)
            .await
            .unwrap();
        let queries = examples()
            .iter()
            .map(|e| e.query)
            .chain(fields().into_iter().map(|f| f.example));
        for query in queries {
            Evaluator::new(query)
                .get_note_ids(&pool)
                .await
                .unwrap_or_else(|e| panic!("{query}: {e}"));
            Evaluator::new(query)
                .get_cards(&pool)
                .await
                .unwrap_or_else(|e| panic!("{query}: {e}"));
        }
    }

    /// The examples about text and combining find what they say.
    #[sqlx::test]
    async fn examples_find_what_they_say(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        create(
            &pool,
            &[
                ("verbs", &["spanish"]),
                ("proverb", &["spanish:verbs"]),
                ("the past tense", &["prehistory"]),
                ("past the tense", &["history:rome"]),
                ("Paris", &["art:history"]),
                ("Rome", &[]),
                ("Paris Rome", &[]),
                ("say \"hi\"", &[]),
                ("singing -ing", &[]),
                ("The or", &["a"]),
                ("a", &["b"]),
                ("b c", &[]),
            ],
        )
        .await;
        let find = async |query| found(&pool, query).await;
        assert_eq!(find("verb").await, ["verbs"]);
        assert_eq!(find("\"past tense\"").await, ["the past tense"]);
        assert_eq!(find("Paris or Rome").await, ["Paris", "Paris Rome", "Rome"]);
        assert_eq!(find("tag=spanish -tag=\"spanish:verbs\"").await, ["verbs"]);
        assert_eq!(
            find("tag~history").await,
            ["Paris", "past the tense", "the past tense"]
        );
        assert_eq!(find("\"-ing\"").await, ["singing -ing"]);
        assert_eq!(find("#\"say \"hi\"\"#").await, ["say \"hi\""]);
        assert_eq!(find("data~re:\"^The \"").await, ["The or"]);
        assert_eq!(find("\"or\"").await, ["The or"]);
        assert_eq!(find("Paris Rome").await, ["Paris Rome"]);
        assert_eq!(find("a or b c").await, ["a", "b c"]);
        let neither = find("-(tag=a or tag=b)").await;
        assert!(neither.contains(&"Rome".to_string()));
        assert!(!neither.contains(&"The or".to_string()) && !neither.contains(&"a".to_string()));
    }
}

use serde_json::json;

use super::*;
use crate::ids::NoteId;
use crate::parsers::CardData;
use crate::parsers::card_tags::CardTagsByOrder;
use crate::parsers::get_cards;
use crate::parsers::typed_answer::expected_typed_answers;

fn parser() -> MarkdownLanguageToolsParser {
    MarkdownLanguageToolsParser::new()
}

#[allow(clippy::needless_pass_by_value, reason = "takes `json!`")]
fn custom_data(value: serde_json::Value) -> CustomData {
    value.as_object().unwrap().clone()
}

fn cards(data: &str) -> Vec<CardData> {
    get_cards(&parser(), None, data, false, false).unwrap()
}

fn request(data: &str, custom_data: CustomData) -> GenerateNoteFilesRequest {
    GenerateNoteFilesRequest {
        note_id: NoteId::from_packed(1 << 8),
        keywords: Vec::new(),
        tags: Vec::new(),
        linked_notes: None,
        note_data: data.to_string(),
        custom_data,
        card_tags: CardTagsByOrder::new(),
    }
}

fn card_file(data: &str, custom_data: CustomData, order: usize, side: CardSide) -> String {
    let cards = cards(data);
    parser().construct_file_data(
        ConstructFileDataType::Card(order, &cards[order - 1], side),
        &request(data, custom_data),
        &NoteImportAction::Update(NoteId::from_packed(1 << 8)),
        false,
    )
}

#[test]
fn test_note_render_source() {
    let source = parser().note_render_source(
        "私は 日本[にほん]が好き。{{[o:1]漢字[かんじ]}} `code[x]` [a link](/a.md) $a[i]$".into(),
    );
    assert_eq!(
        source,
        "私は`<ruby>日本<rt>にほん</rt></ruby>`{=html}`\\ruby{日本}{にほん}`{=latex}が好き。\\[`<ruby>漢字<rt>かんじ</rt></ruby>`{=html}`\\ruby{漢字}{かんじ}`{=latex}\\] `code[x]` [a link](/a.md) $a[i]$"
    );
    // A note without readings is the Markdown parser's
    let plain = "No readings, {{[o:1]Paris}} and [a link](/a.md)".to_string();
    assert_eq!(
        parser().note_render_source(plain.clone()),
        MarkdownParser::new().note_render_source(plain)
    );
}

#[test]
fn test_card_files() {
    let data = "{{[o:1]漢字[かんじ]}} means kanji. 中文[zhōngwén]";
    let front = card_file(data, CustomData::default(), 1, CardSide::Front);
    assert_eq!(front, "[card-1 _____]{.mark} means kanji. 中文[zhōngwén]");
    let back = card_file(data, CustomData::default(), 1, CardSide::Back);
    assert_eq!(
        back,
        "[card-1 漢字[かんじ]]{.mark} means kanji. 中文[zhōngwén]"
    );
    // The reading in a revealed cloze is rendered
    assert!(
        parser()
            .note_render_source(back)
            .starts_with("[card-1 `<ruby>漢字")
    );
}

#[test]
fn test_readings_on_back() {
    let data = "漢字[かんじ] is {{[o:1]kanji}}";
    let on_back = custom_data(json!({"readings": "back"}));
    assert_eq!(
        card_file(data, on_back.clone(), 1, CardSide::Front),
        "漢字 is [card-1 _____]{.mark}"
    );
    assert_eq!(
        card_file(data, on_back, 1, CardSide::Back),
        "漢字[かんじ] is [card-1 kanji]{.mark}"
    );
    assert_eq!(
        card_file(data, CustomData::default(), 1, CardSide::Front),
        "漢字[かんじ] is [card-1 _____]{.mark}"
    );
}

#[test]
fn test_language() {
    let data = "{{[o:1]中文[zhōngwén]}}";
    let file = card_file(
        data,
        custom_data(json!({"language": "zh"})),
        1,
        CardSide::Front,
    );
    assert!(
        file.starts_with("<!--- spares: language zh --->\n"),
        "{file}"
    );
    assert_eq!(file_language(&file).unwrap().code, "zh");
    // A note's file has its custom data
    let note = "<!--- # custom-data: {\"language\": \"zh\"} --->\n漢字";
    assert_eq!(file_language(note).unwrap().code, "zh");
    // Or else its script
    assert_eq!(file_language("ひらがな").unwrap().code, "ja");
    assert!(file_language("漢字").is_none());
    let args = |contents, pdf| {
        pandoc_args(contents, pdf)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        args(&file, true),
        [
            "--pdf-engine=lualatex",
            "--metadata=CJKmainfont=FandolSong-Regular.otf",
            "--metadata=CJKoptions=BoldFont=FandolSong-Bold.otf",
        ]
    );
    assert_eq!(args(&file, false), ["--metadata=lang=zh"]);
    assert_eq!(args("English", true), ["--pdf-engine=lualatex"]);
}

#[test]
fn test_typed_answers() {
    let blanks = |data: &str| {
        expected_typed_answers(&parser(), &cards(data)[0])
            .unwrap()
            .into_iter()
            .map(|blank| (blank.expected, blank.also_accepted))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        blanks("{{[o:1;t]食[た]べる}}"),
        [("食べる".to_string(), vec!["たべる".to_string()])]
    );
    assert_eq!(
        blanks("{{[o:1;t]中文[zhōngwén]}}"),
        [("中文".to_string(), vec!["zhōngwén".to_string()])]
    );
    assert_eq!(
        blanks("{{[o:1;t]*kanji*}}"),
        [("kanji".to_string(), vec![])]
    );
    // The typed answer is compared with the spelling it matches
    let blank = &expected_typed_answers(&parser(), &cards("{{[o:1;t]食[た]べる}}")[0]).unwrap()[0];
    assert_eq!(blank.answer_for("たべる", false, false), "たべる");
    assert_eq!(blank.answer_for("食べる", false, false), "食べる");
    assert_eq!(blank.answer_for("たべ", false, false), "食べる");
}

#[test]
fn test_spoken_text() {
    let parser = parser();
    // Kana readings are spoken, pinyin isn't
    assert_eq!(
        parser.spoken_text("私は 日本[にほん]に いる"),
        "私はにほんに いる"
    );
    assert_eq!(parser.spoken_text("我学 中文[zhōngwén]"), "我学中文");
    assert_eq!(parser.spoken_text("**한국어**"), "한국어");
}

/// The HTML shows readings as `<ruby>`, and the page has the note's language.
#[test]
fn test_render_web_file() {
    let dir = std::env::temp_dir().join(format!("spares-language-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("0001.md");
    std::fs::write(
        &text,
        "<!--- spares: language ja --->\n私は 日本[にほん]が好き。[link](https://example.com)\n",
    )
    .unwrap();
    let html = dir.join("0001.html");
    parser()
        .render_web_file(
            &text,
            &dir.join("0001.pdf"),
            &html,
            WebRenderMode::Auto,
            &mut RenderDependencies::default(),
        )
        .unwrap();
    let html = std::fs::read_to_string(html).unwrap();
    assert!(html.contains("<ruby>日本<rt>にほん</rt></ruby>"), "{html}");
    assert!(html.contains(r#"lang="ja""#), "{html}");
    assert!(!html.contains(r"\ruby"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The default template's LaTeX is only for the PDF, so the HTML of a note made from it loses
/// nothing and isn't replaced by the PDF.
#[test]
fn test_template_web_render() {
    let dir = std::env::temp_dir().join(format!(
        "spares-language-tools-template-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("0001.md");
    let body = parser()
        .default_template()
        .replace("<!--- spares: body --->", "日本[にほん]");
    std::fs::write(&text, body).unwrap();
    parser()
        .render_web_file(
            &text,
            &dir.join("0001.pdf"),
            &dir.join("0001.html"),
            WebRenderMode::Auto,
            &mut RenderDependencies::default(),
        )
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The PDF is typeset by lualatex, with the note's CJK font. Skipped without lualatex.
#[test]
fn test_render_file() {
    if std::process::Command::new("kpsewhich")
        .arg("luatexja-ruby.sty")
        .output()
        .map_or(true, |output| output.stdout.is_empty())
    {
        eprintln!("skipped: no luatexja-ruby");
        return;
    }
    let dir =
        std::env::temp_dir().join(format!("spares-language-tools-pdf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("0001.md");
    let template = parser().default_template();
    let body = template.replace(
        "<!--- spares: body --->",
        "<!--- spares: language zh --->\n# 中文\n\n我学 **中文[zhōngwén]**。",
    );
    std::fs::write(&text, body).unwrap();
    let pdf = dir.join("0001.pdf");
    let rendered = parser().render_file(&dir, &text, &dir, &pdf).unwrap();
    assert!(
        rendered.output.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.output.stderr)
    );
    // The font's name is in compressed streams, which poppler's pdffonts reads
    if let Ok(fonts) = std::process::Command::new("pdffonts").arg(&pdf).output() {
        let fonts = String::from_utf8_lossy(&fonts.stdout);
        assert!(fonts.contains("FandolSong-Regular"), "{fonts}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file of this parser's notes is told from the Markdown parser's when importing it.
#[test]
fn test_files_name_the_parser() {
    use crate::parsers::collect_blocks;
    let note_id = NoteId::from_packed(1 << 8);
    let request = request("漢字[かんじ] is {{kanji}}", CustomData::default());
    let file = parser().construct_full_file_data(
        &[(ConstructFileDataType::Note, &request)],
        &NoteImportAction::Update(note_id),
    );
    assert!(
        file.starts_with("<!--- spares: start markdown-language-tools --->\n"),
        "{file}"
    );
    assert_eq!(collect_blocks(&parser(), &file).unwrap().len(), 1);
    assert_eq!(
        collect_blocks(&MarkdownParser::new(), &file).unwrap(),
        [""; 0]
    );
    let markdown_file = MarkdownParser::new().construct_full_file_data(
        &[(ConstructFileDataType::Note, &request)],
        &NoteImportAction::Update(note_id),
    );
    assert_eq!(collect_blocks(&parser(), &markdown_file).unwrap(), [""; 0]);
}

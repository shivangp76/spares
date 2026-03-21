use crate::parsers::generate_files::CardSide;
use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable, get_cards,
    image_occlusion::{
        ImageOcclusionConfig, ImageOcclusionData,
        construct::{get_clozes_from_svg, modify_clozes_for_card},
        create_image_occlusion_cards, get_image_occlusion_directory,
        get_image_occlusion_rendered_directory,
    },
    impls::markdown::MarkdownParser,
};
use indoc::indoc;
use pretty_assertions::assert_eq;
use std::{fs::read_to_string, path::PathBuf, sync::Arc, time::Instant};
use xmltree::{Element, EmitterConfig};

const MOVE_FILES: bool = false;

#[test]
fn test_get_cards_image_occlusion_1() {
    // Tests
    // - Basics
    // - Multiple image occlusion files
    // - Image 1: 2 clozes, each with default settings
    // - Image 2: 2 clozes, grouped together
    let seed = "A";
    let image_1_file_stem = format!("test-{}-1", seed);
    let image_2_file_stem = format!("test-{}-2", seed);

    // let temp_dir = std::env::temp_dir();
    let temp_dir = get_image_occlusion_directory();
    let mut original_image_filepath_1 = temp_dir.clone();
    original_image_filepath_1.push(format!("{}.svg", image_1_file_stem));
    let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 124 124" fill="none"><rect width="124" height="124" rx="24" fill="#F97316"/></svg>"##;
    std::fs::write(&original_image_filepath_1, text).unwrap();
    // class="layer" is for svgedit
    let clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" />
          </g>
        </svg>"## };
    let mut clozes_filepath_1 = temp_dir.clone();
    clozes_filepath_1.push(format!("{}_clozes.svg", image_1_file_stem));
    std::fs::write(&clozes_filepath_1, clozes_filedata_1).unwrap();

    let mut original_image_filepath_2 = temp_dir.clone();
    original_image_filepath_2.push(format!("{}.svg", image_2_file_stem));
    let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 124 124" fill="none"><rect width="124" height="124" rx="24" fill="#F97316"/></svg>"##;
    std::fs::write(&original_image_filepath_2, text).unwrap();
    let clozes_filedata_2 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" data-cloze-settings="g:1" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
          </g>
        </svg>"## };
    let mut clozes_filepath_2 = temp_dir.clone();
    clozes_filepath_2.push(format!("{}_clozes.svg", image_2_file_stem));
    std::fs::write(&clozes_filepath_2, clozes_filedata_2).unwrap();

    let note_data = format!(
        indoc! { "a
        <!--- spares: image occlusion start --->
        <!--- original_image_filepath = \"{}\" --->
        <!--- clozes_filepath = \"{}\" --->
        <!--- front_conceal = \"only_grouping\" --->
        <!--- back_reveal = \"full_note\" --->
        <!--- back_emphasis = false --->
        <!--- spares: image occlusion end --->
        b
        <!--- spares: image occlusion start --->
        <!--- original_image_filepath = \"{}\" --->
        <!--- clozes_filepath = \"{}\" --->
        <!--- front_conceal = \"only_grouping\" --->
        <!--- back_reveal = \"full_note\" --->
        <!--- back_emphasis = false --->
        [Image Occlusion](/some/random/image/path)
        <!--- spares: image occlusion end --->
        c" },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
        original_image_filepath_2.display(),
        clozes_filepath_2.display(),
    );

    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, note_data.as_str(), true, MOVE_FILES);
    assert!(cards_res.is_ok());
    let image_occlusion_1 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-1.svg"),
        clozes_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-1_clozes.svg"),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
    });
    let image_occlusion_2 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-2.svg"),
        clozes_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-2_clozes.svg"),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
    });
    if let Ok(cards) = cards_res {
        let expected = vec![
                CardData {
                    order: Some(1),
                    previous_order: None,
                    grouping: ClozeGrouping::Auto(1),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_emphasis: false,
                    back_type: BackType::NoteFilePath,
                    inherit: None,
                    data: vec![
                        NotePart::SurroundingData("a\n".to_string()),
                        NotePart::ImageOcclusion { cloze_indices: vec![(0, ClozeHiddenReplacement::ToAnswer{ hint: None })], data: image_occlusion_1.clone() },
                        NotePart::SurroundingData("b\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-2.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-2_clozes.svg\" --->\n<!--- front_conceal = \"only_grouping\" --->\n<!--- back_reveal = \"full_note\" --->\n<!--- back_emphasis = false --->\n![Test A 2](/tmp/spares/data/image_occlusions/test-A-2.svg)\n<!--- spares: image occlusion end --->\nc".to_string()),
                    ],
                },
                CardData {
                    order: Some(2),
                    previous_order: None,
                    grouping: ClozeGrouping::Auto(2),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_emphasis: false,
                    back_type: BackType::NoteFilePath,
                    inherit: None,
                    data: vec![
                        NotePart::SurroundingData("a\n".to_string()),
                        NotePart::ImageOcclusion { cloze_indices: vec![(1, ClozeHiddenReplacement::ToAnswer{ hint: None })], data: image_occlusion_1.clone() },
                        NotePart::SurroundingData("b\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-2.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-2_clozes.svg\" --->\n<!--- front_conceal = \"only_grouping\" --->\n<!--- back_reveal = \"full_note\" --->\n<!--- back_emphasis = false --->\n![Test A 2](/tmp/spares/data/image_occlusions/test-A-2.svg)\n<!--- spares: image occlusion end --->\nc".to_string()),
                    ],
                },
                CardData {
                    order: Some(3),
                    previous_order: None,
                    grouping: ClozeGrouping::Custom("1".to_string()),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_emphasis: false,
                    back_type: BackType::NoteFilePath,
                    inherit: None,
                    data: vec![
                        NotePart::SurroundingData("a\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-1.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-1_clozes.svg\" --->\n<!--- front_conceal = \"only_grouping\" --->\n<!--- back_reveal = \"full_note\" --->\n<!--- back_emphasis = false --->\n![Test A 1](/tmp/spares/data/image_occlusions/test-A-1.svg)\n<!--- spares: image occlusion end --->\nb\n".to_string()),
                        // Both clozes are combined into 1 ImageOcclusionData, since they are in the same group
                        NotePart::ImageOcclusion { cloze_indices: vec![(0, ClozeHiddenReplacement::ToAnswer{ hint: None }), (1, ClozeHiddenReplacement::NotToAnswer)], data: image_occlusion_2.clone() },
                        NotePart::SurroundingData("c".to_string()),
                    ],
                },
            ];
        assert_eq!(cards, expected);
    }
    // Verify the clozes files were updated to include the new orders
    // 1
    let expected_new_clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" data-cloze-settings="o:1" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="o:2" />
          </g>
        </svg>"## };
    let clozes_filepath = PathBuf::from("/tmp/spares/data/image_occlusions/test-A-1_clozes.svg");
    let new_clozes_filedata_1 = read_to_string(&clozes_filepath).unwrap();
    assert_eq!(new_clozes_filedata_1, expected_new_clozes_filedata_1);
    // 2
    let expected_new_clozes_filedata_2 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" data-cloze-settings="g:1;o:3" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
          </g>
        </svg>"## };
    let clozes_filepath = PathBuf::from("/tmp/spares/data/image_occlusions/test-A-2_clozes.svg");
    let new_clozes_filedata_2 = read_to_string(&clozes_filepath).unwrap();
    assert_eq!(new_clozes_filedata_2, expected_new_clozes_filedata_2);
}

#[test]
fn test_get_cards_image_occlusion_2() {
    // This tests:
    // - Clozes that are hidden, but don't need to be answered are color differently
    // - Clozes have their colors overridden on cards
    // - Hints are properly rendered in cards
    // - Adding a text cloze with grouping 1 and an image cloze with grouping 1. Makes sure that all clozes get their settings updated properly. Makes sure that cloze settings boil up between image occlusions and text clozes.
    //
    // Create an image occlusion image
    let seed = "hint";
    let image_1_file_stem = format!("test-{}-1", seed);
    let temp_dir = get_image_occlusion_directory();
    let mut original_image_filepath_1 = temp_dir.clone();
    original_image_filepath_1.push(format!("{}.svg", image_1_file_stem));
    let text = indoc! { r##"
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" viewBox="0 0 124 124" fill="none">
          <rect width="124" height="124" rx="24" fill="#F97316"/>
        </svg>"##
    };
    std::fs::write(&original_image_filepath_1, text).unwrap();
    let clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="blue" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" data-cloze-settings="g:1;h:Hi there;s:" />
            <ellipse fill="blue" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:"/>
          </g>
        </svg>"## };
    let mut clozes_filepath_1 = temp_dir.clone();
    clozes_filepath_1.push(format!("{}_clozes.svg", image_1_file_stem));
    std::fs::write(&clozes_filepath_1, clozes_filedata_1).unwrap();

    // Construct note data
    let note_data = format!(
        indoc! { "
            a{{{{[g:1]b}}}}
            <!--- spares: image occlusion start --->
            <!--- original_image_filepath = \"{}\" --->
            <!--- clozes_filepath = \"{}\" --->
            <!--- front_conceal = \"only_grouping\" --->
            <!--- back_reveal = \"full_note\" --->
            <!--- back_emphasis = false --->
            <!--- spares: image occlusion end --->
            " },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
    );

    // Get cards
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, note_data.as_str(), true, MOVE_FILES);
    assert!(cards_res.is_ok());

    let image_occlusion_1 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1.svg",
            seed
        )),
        clozes_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1_clozes.svg",
            seed
        )),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
    });
    let cards = cards_res.unwrap();
    let expected = vec![CardData {
        order: Some(1),
        previous_order: None,
        grouping: ClozeGrouping::Custom("1".to_string()),
        is_suspended: Some(true),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
        back_type: BackType::NoteFilePath,
        inherit: None,
        data: vec![
            NotePart::SurroundingData("a".to_string()),
            NotePart::ClozeStart("{{[g:1;o:1]".to_string()),
            NotePart::ClozeData(
                "b".to_string(),
                ClozeHiddenReplacement::ToAnswer { hint: None },
            ),
            NotePart::ClozeEnd("}}".to_string()),
            NotePart::SurroundingData("\n".to_string()),
            NotePart::ImageOcclusion {
                cloze_indices: vec![
                    (
                        0,
                        ClozeHiddenReplacement::ToAnswer {
                            hint: Some("Hi there".to_string()),
                        },
                    ),
                    (1, ClozeHiddenReplacement::NotToAnswer),
                ],
                data: image_occlusion_1.clone(),
            },
        ],
    }];
    assert_eq!(cards, expected);

    // Verify the clozes files was updated
    let expected_new_clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="blue" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" data-cloze-settings="h:Hi there;g:1" />
            <ellipse fill="blue" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
          </g>
        </svg>"## };
    let clozes_filepath = PathBuf::from(format!(
        "/tmp/spares/data/image_occlusions/test-{}-1_clozes.svg",
        seed
    ));
    let new_clozes_filedata_1 = read_to_string(&clozes_filepath).unwrap();
    assert_eq!(new_clozes_filedata_1, expected_new_clozes_filedata_1);

    // Verify the card is created correctly
    let temp_cloze_indices = &cards[0]
        .data
        .iter()
        .filter_map(|x| match x {
            NotePart::ImageOcclusion {
                cloze_indices,
                data: _,
            } => Some(cloze_indices),
            _ => None,
        })
        .collect::<Vec<_>>();
    let cloze_indices = temp_cloze_indices[0];
    let mut clozes_svg_element = Element::parse(new_clozes_filedata_1.as_bytes()).unwrap();
    let mut clozes = get_clozes_from_svg(&mut clozes_svg_element).unwrap();
    let config = ImageOcclusionConfig::default();
    modify_clozes_for_card(
        &cloze_indices,
        &mut clozes,
        image_occlusion_1.front_conceal,
        image_occlusion_1.back_reveal,
        false,
        CardSide::Front,
        &config,
    );
    let mut buffer: Vec<u8> = Vec::new();
    let _ = clozes_svg_element
        .write_with_config(&mut buffer, EmitterConfig::new().perform_indent(true));
    let card_cloze_data = String::from_utf8(buffer).unwrap();
    let expected_card_cloze_data = indoc! {
        r##"<?xml version="1.0" encoding="UTF-8"?>
            <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
              <g class="layer" id="markup-group">
                <title>Markup</title>
              </g>
              <g class="layer" id="clozes-group">
                <title>Clozes</title>
                <g>
                  <rect fill="#FF7E7E" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" data-cloze-settings="h:Hi there;g:1" />
                  <text font-size="16" text-anchor="middle" dominant-baseline="middle" x="115.28571500000001" y="110.67857">Hi there</text>
                </g>
                <g>
                  <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
                  <text font-size="16" text-anchor="middle" dominant-baseline="middle" x="346.52633" y="78.94737">(no answer)</text>
                </g>
              </g>
            </svg>"##
    };
    assert_eq!(card_cloze_data, expected_card_cloze_data);
}

#[test]
fn test_get_cards_image_occlusion_front_conceal() {
    // Tests
    // - Front conceal
    // - When the back reveal is only answered with emphasis, clozes that are answered are emphasized
    // Create an image occlusion image
    let seed = "special-type";
    let image_1_file_stem = format!("test-{}-1", seed);
    let temp_dir = get_image_occlusion_directory();
    let mut original_image_filepath_1 = temp_dir.clone();
    original_image_filepath_1.push(format!("{}.svg", image_1_file_stem));
    let text = indoc! { r##"
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" viewBox="0 0 124 124" fill="none">
          <rect width="124" height="124" rx="24" fill="#F97316"/>
        </svg>"##
    };
    std::fs::write(&original_image_filepath_1, text).unwrap();
    let clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
             <rect fill="blue" height="75" id="svg_1" stroke="#2D2D2D" width="123.21" x="53.68" y="65.18"  data-cloze-settings="g:1" />
             <rect fill="blue" height="75" id="svg_2" stroke="#2D2D2D" width="123.21" x="193.68" y="236.18" data-cloze-settings="g:1;h:Hi" />
             <ellipse cx="346.53" cy="78.95" fill="blue" id="svg_3" rx="46.32" ry="46.32" stroke="#2D2D2D" stroke-dasharray="null" stroke-linecap="null" stroke-linejoin="null" data-cloze-settings="" />
          </g>
        </svg>"## };
    let mut clozes_filepath_1 = temp_dir.clone();
    clozes_filepath_1.push(format!("{}_clozes.svg", image_1_file_stem));
    std::fs::write(&clozes_filepath_1, clozes_filedata_1).unwrap();

    // Construct note data
    let note_data = format!(
        indoc! { "
            a{{{{[g:1]b}}}}
            <!--- spares: image occlusion start --->
            <!--- original_image_filepath = \"{}\" --->
            <!--- clozes_filepath = \"{}\" --->
            <!--- front_conceal = \"all_groupings\" --->
            <!--- back_reveal = \"only_answered\" --->
            <!--- back_emphasis = true --->
            <!--- spares: image occlusion end --->
            " },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
    );

    // Get cards
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, note_data.as_str(), true, MOVE_FILES);
    assert!(cards_res.is_ok());

    let back_emphasis = true;
    let image_occlusion_1 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1.svg",
            seed
        )),
        clozes_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1_clozes.svg",
            seed
        )),
        front_conceal: FrontConceal::AllGroupings,
        back_reveal: BackReveal::OnlyAnswered,
        back_emphasis,
    });
    let cards = cards_res.unwrap();
    let expected = vec![
        CardData {
            order: Some(1),
            previous_order: None,
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_emphasis,
            back_type: BackType::CardFilePath,
            inherit: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;o:1;f:all;b:a;be:true]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("\n".to_string()),
                NotePart::ImageOcclusion {
                    cloze_indices: vec![
                        (0, ClozeHiddenReplacement::ToAnswer { hint: None }),
                        (
                            1,
                            ClozeHiddenReplacement::ToAnswer {
                                hint: Some("Hi".to_string()),
                            },
                        ),
                    ],
                    data: image_occlusion_1.clone(),
                },
            ],
        },
        CardData {
            order: Some(2),
            previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_emphasis,
            back_type: BackType::CardFilePath,
            inherit: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;o:1;f:all;b:a;be:true]".to_string()),
                NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("\n".to_string()),
                NotePart::ImageOcclusion {
                    cloze_indices: vec![(2, ClozeHiddenReplacement::ToAnswer { hint: None })],
                    data: image_occlusion_1.clone(),
                },
            ],
        },
    ];
    assert_eq!(cards, expected);

    // Verify the clozes files was updated
    let expected_new_clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="blue" height="75" id="svg_1" stroke="#2D2D2D" width="123.21" x="53.68" y="65.18" data-cloze-settings="g:1" />
            <rect fill="blue" height="75" id="svg_2" stroke="#2D2D2D" width="123.21" x="193.68" y="236.18" data-cloze-settings="h:Hi;g:1" />
            <ellipse cx="346.53" cy="78.95" fill="blue" id="svg_3" rx="46.32" ry="46.32" stroke="#2D2D2D" stroke-dasharray="null" stroke-linecap="null" stroke-linejoin="null" data-cloze-settings="o:2" />
          </g>
        </svg>"## };
    let clozes_filepath = PathBuf::from(format!(
        "/tmp/spares/data/image_occlusions/test-{}-1_clozes.svg",
        seed
    ));
    let new_clozes_filedata_1 = read_to_string(&clozes_filepath).unwrap();
    assert_eq!(new_clozes_filedata_1, expected_new_clozes_filedata_1);

    // Verify the card cloze front file is created correctly
    let temp_cloze_indices = &cards[0]
        .data
        .iter()
        .filter_map(|x| match x {
            NotePart::ImageOcclusion {
                cloze_indices,
                data: _,
            } => Some(cloze_indices),
            _ => None,
        })
        .collect::<Vec<_>>();
    let cloze_indices = temp_cloze_indices[0];
    let mut clozes_svg_element = Element::parse(new_clozes_filedata_1.as_bytes()).unwrap();
    let mut clozes = get_clozes_from_svg(&mut clozes_svg_element).unwrap();
    let config = ImageOcclusionConfig::default();
    modify_clozes_for_card(
        &cloze_indices,
        &mut clozes,
        image_occlusion_1.front_conceal,
        image_occlusion_1.back_reveal,
        back_emphasis,
        CardSide::Front,
        &config,
    );
    let mut buffer: Vec<u8> = Vec::new();
    let _ = clozes_svg_element
        .write_with_config(&mut buffer, EmitterConfig::new().perform_indent(true));
    let card_cloze_data = String::from_utf8(buffer).unwrap();
    let expected_card_cloze_data = indoc! {
        r##"<?xml version="1.0" encoding="UTF-8"?>
            <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
              <g class="layer" id="markup-group">
                <title>Markup</title>
              </g>
              <g class="layer" id="clozes-group">
                <title>Clozes</title>
                <rect fill="#FF7E7E" height="75" id="svg_1" stroke="#2D2D2D" width="123.21" x="53.68" y="65.18" data-cloze-settings="g:1" />
                <g>
                  <rect fill="#FF7E7E" height="75" id="svg_2" stroke="#2D2D2D" width="123.21" x="193.68" y="236.18" data-cloze-settings="h:Hi;g:1" />
                  <text font-size="16" text-anchor="middle" dominant-baseline="middle" x="255.285" y="281.68">Hi</text>
                </g>
                <g>
                  <ellipse cx="346.53" cy="78.95" fill="#FFEBA2" id="svg_3" rx="46.32" ry="46.32" stroke="#2D2D2D" stroke-dasharray="null" stroke-linecap="null" stroke-linejoin="null" data-cloze-settings="o:2" />
                  <text font-size="16" text-anchor="middle" dominant-baseline="middle" x="346.53" y="78.95">(no answer)</text>
                </g>
              </g>
            </svg>"##
    };
    assert_eq!(card_cloze_data, expected_card_cloze_data);

    // Verify the card cloze back file is created correctly
    let temp_cloze_indices = &cards[0]
        .data
        .iter()
        .filter_map(|x| match x {
            NotePart::ImageOcclusion {
                cloze_indices,
                data: _,
            } => Some(cloze_indices),
            _ => None,
        })
        .collect::<Vec<_>>();
    let cloze_indices = temp_cloze_indices[0];
    let mut clozes_svg_element = Element::parse(new_clozes_filedata_1.as_bytes()).unwrap();
    let mut clozes = get_clozes_from_svg(&mut clozes_svg_element).unwrap();
    modify_clozes_for_card(
        &cloze_indices,
        &mut clozes,
        image_occlusion_1.front_conceal,
        image_occlusion_1.back_reveal,
        back_emphasis,
        CardSide::Back,
        &config,
    );
    let mut buffer: Vec<u8> = Vec::new();
    let _ = clozes_svg_element
        .write_with_config(&mut buffer, EmitterConfig::new().perform_indent(true));
    let card_back_cloze_data = String::from_utf8(buffer).unwrap();
    let expected_card_back_cloze_data = indoc! {
        r##"<?xml version="1.0" encoding="UTF-8"?>
            <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
              <g class="layer" id="markup-group">
                <title>Markup</title>
              </g>
              <g class="layer" id="clozes-group">
                <title>Clozes</title>
                <rect fill="#FF7E7E" height="75" id="svg_1" stroke="#2D2D2D" width="123.21" x="53.68" y="65.18" data-cloze-settings="g:1" fill-opacity="0.3" />
                <rect fill="#FF7E7E" height="75" id="svg_2" stroke="#2D2D2D" width="123.21" x="193.68" y="236.18" data-cloze-settings="h:Hi;g:1" fill-opacity="0.3" />
                <ellipse cx="346.53" cy="78.95" fill="blue" id="svg_3" rx="46.32" ry="46.32" stroke="#2D2D2D" stroke-dasharray="null" stroke-linecap="null" stroke-linejoin="null" data-cloze-settings="o:2" />
              </g>
            </svg>"##
    };
    assert_eq!(card_back_cloze_data, expected_card_back_cloze_data);
}

#[test]
fn test_get_cards_image_occlusion_grouping() {
    // Tests:
    // - Creating a text cloze with no settings and an image occlusion cloze with no settings should create 2 cards (1 per cloze, since they are not grouped)
    //
    // Create an image occlusion image
    let seed = "grouping";
    let image_1_file_stem = format!("test-{}-1", seed);
    let temp_dir = get_image_occlusion_directory();
    let mut original_image_filepath_1 = temp_dir.clone();
    original_image_filepath_1.push(format!("{}.svg", image_1_file_stem));
    let text = indoc! { r##"
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" viewBox="0 0 124 124" fill="none">
          <rect width="124" height="124" rx="24" fill="#F97316"/>
        </svg>"##
    };
    std::fs::write(&original_image_filepath_1, text).unwrap();
    let clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="800" height="400">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
             <rect fill="blue" height="75" id="svg_1" stroke="#2D2D2D" width="123.21" x="53.68" y="65.18"  data-cloze-settings="" />
          </g>
        </svg>"## };
    let mut clozes_filepath_1 = temp_dir.clone();
    clozes_filepath_1.push(format!("{}_clozes.svg", image_1_file_stem));
    std::fs::write(&clozes_filepath_1, clozes_filedata_1).unwrap();

    // Construct note data
    let note_data = format!(
        indoc! { "
            a{{{{b}}}}
            <!--- spares: image occlusion start --->
            <!--- original_image_filepath = \"{}\" --->
            <!--- clozes_filepath = \"{}\" --->
            <!--- front_conceal = \"all_groupings\" --->
            <!--- back_reveal = \"only_answered\" --->
            <!--- back_emphasis = false --->
            <!--- spares: image occlusion end --->
            " },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
    );

    // Get cards
    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, note_data.as_str(), true, MOVE_FILES);
    assert!(cards_res.is_ok());

    let image_occlusion_1 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1.svg",
            seed
        )),
        clozes_filepath: PathBuf::from(format!(
            "/tmp/spares/data/image_occlusions/test-{}-1_clozes.svg",
            seed
        )),
        front_conceal: FrontConceal::AllGroupings,
        back_reveal: BackReveal::OnlyAnswered,
        back_emphasis: false,
    });
    let cards = cards_res.unwrap();
    let expected = vec![
        CardData {
            order: Some(1),
                    previous_order: None,
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
            back_type: BackType::NoteFilePath,
            inherit: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[o:1]".to_string()),
                NotePart::ClozeData(
                    "b".to_string(),
                    ClozeHiddenReplacement::ToAnswer { hint: None },
                ),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-grouping-1.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-grouping-1_clozes.svg\" --->\n<!--- front_conceal = \"all_groupings\" --->\n<!--- back_reveal = \"only_answered\" --->\n<!--- back_emphasis = false --->\n![Test Grouping 1](/tmp/spares/data/image_occlusions/test-grouping-1.svg)\n<!--- spares: image occlusion end --->\n".to_string()),
            ],
        },
        CardData {
            order: Some(2),
                    previous_order: None,
            grouping: ClozeGrouping::Auto(2),
            is_suspended: None,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_emphasis: false,
            back_type: BackType::CardFilePath,
            inherit: None,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[o:1]".to_string()),
                NotePart::ClozeData("b".to_string(), ClozeHiddenReplacement::NotToAnswer),
                NotePart::ClozeEnd("}}".to_string()),
                NotePart::SurroundingData("\n".to_string()),
                NotePart::ImageOcclusion {
                    cloze_indices: vec![(0, ClozeHiddenReplacement::ToAnswer { hint: None })],
                    data: image_occlusion_1.clone(),
                },
            ],
        },
    ];
    assert_eq!(cards, expected);
}

#[test]
fn test_image_occlusion_parallel_performance() {
    // Test that parallel processing is faster than sequential processing
    // for multiple image occlusion cards

    use crate::config::read_external_config;
    use crate::parsers::image_occlusion::construct::create_image_occlusion_card;
    use crate::parsers::image_occlusion::get_image_occlusion_card_filepath;

    let seed = "perf-test";
    let num_cards = 8; // Create 8 cards to test parallelism

    let temp_dir = get_image_occlusion_directory();
    let rendered_dir = get_image_occlusion_rendered_directory();

    // Create test image occlusion files
    let mut image_occlusions = Vec::new();
    for i in 1..=num_cards {
        let file_stem = format!("{}-{}", seed, i);
        let mut original_image_filepath = temp_dir.clone();
        original_image_filepath.set_extension("");
        original_image_filepath.push(format!("{}.png", file_stem));

        // Create a simple PNG image (400x400 orange rectangle)
        use image::{Rgba, RgbaImage};
        let mut img = RgbaImage::new(400, 400);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([249, 115, 22, 255]); // Orange color
        }
        img.save(&original_image_filepath).unwrap();

        let mut clozes_filepath = temp_dir.clone();
        clozes_filepath.push(format!("{}_clozes.svg", file_stem));

        // Create clozes SVG
        let clozes_svg = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" />
          </g>
        </svg>"##
        );
        std::fs::write(&clozes_filepath, clozes_svg).unwrap();

        let image_occlusion = Arc::new(ImageOcclusionData {
            original_image_filepath: original_image_filepath.clone(),
            clozes_filepath: clozes_filepath.clone(),
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
        });
        image_occlusions.push((
            vec![(0, ClozeHiddenReplacement::ToAnswer { hint: None })],
            image_occlusion,
        ));
    }

    // Create card data
    let card_data = CardData {
        order: Some(1),
        previous_order: None,
        grouping: ClozeGrouping::Auto(1),
        is_suspended: None,
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_emphasis: false,
        back_type: BackType::NoteFilePath,
        inherit: None,
        data: image_occlusions
            .iter()
            .map(|(cloze_indices, data)| NotePart::ImageOcclusion {
                cloze_indices: cloze_indices.clone(),
                data: data.clone(),
            })
            .collect(),
    };

    // Ensure output directory exists
    std::fs::create_dir_all(&rendered_dir).unwrap();

    let mut output_path = rendered_dir.clone();
    output_path.push("perf-test-card-front");
    output_path.set_extension("txt"); // append_to_stem requires an extension

    // Test parallel version
    let parallel_start = Instant::now();
    let parallel_result = create_image_occlusion_cards(&card_data, CardSide::Front, &output_path);
    let parallel_duration = parallel_start.elapsed();

    if let Err(e) = parallel_result {
        // List what files actually exist
        let files: Vec<_> = std::fs::read_dir(&rendered_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        panic!(
            "Parallel processing failed: {:?}. Files in dir: {:?}",
            e, files
        );
    }

    // Verify files were created
    for i in 1..=num_cards {
        let card_filepath = get_image_occlusion_card_filepath(&output_path, CardSide::Front, i);
        if !card_filepath.exists() {
            // List what files actually exist
            let files: Vec<_> = std::fs::read_dir(&rendered_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            panic!(
                "Card file {} should exist at {:?}. Files in dir: {:?}",
                i, card_filepath, files
            );
        }
        // Clean up
        let _ = std::fs::remove_file(&card_filepath);
    }

    // Test sequential version for comparison
    let config = read_external_config().unwrap();
    let sequential_start = Instant::now();

    let image_occlusions_seq: Vec<_> = card_data
        .data
        .iter()
        .filter_map(|note_part| match note_part {
            NotePart::ImageOcclusion {
                cloze_indices,
                data,
            } => Some((cloze_indices, data)),
            _ => None,
        })
        .enumerate()
        .collect();

    for (i, (cloze_indices, image_occlusion_data)) in image_occlusions_seq.iter() {
        let image_occlusion_order_in_card = i + 1;
        let card_filepath = get_image_occlusion_card_filepath(
            &output_path,
            CardSide::Front,
            image_occlusion_order_in_card,
        );
        let _ = create_image_occlusion_card(
            cloze_indices,
            image_occlusion_data,
            &card_filepath,
            CardSide::Front,
            &config.image_occlusion,
        );
    }

    let sequential_duration = sequential_start.elapsed();

    // Clean up sequential test files
    for i in 1..=num_cards {
        let card_filepath = get_image_occlusion_card_filepath(&output_path, CardSide::Front, i);
        let _ = std::fs::remove_file(&card_filepath);
    }

    // Assert that parallel processing is faster (or at least not significantly slower)
    // With multiple cards, parallel should show improvement
    println!(
        "Parallel: {:?}, Sequential: {:?}, Speedup: {:.2}x",
        parallel_duration,
        sequential_duration,
        sequential_duration.as_secs_f64() / parallel_duration.as_secs_f64()
    );

    // Parallel should be faster or at least not more than 10% slower
    // (to account for thread overhead on systems with few cores)
    assert!(
        parallel_duration <= sequential_duration * 11 / 10,
        "Parallel processing should be faster than sequential. Parallel: {:?}, Sequential: {:?}",
        parallel_duration,
        sequential_duration
    );
}

// ── Grouped-shape tests ───────────────────────────────────────────────────────

/// A `<g>` element inside `clozes-group` is accepted as a single cloze shape.
/// Mixed SVGs (group + primitive) produce the correct count.
#[test]
fn test_grouped_shapes_accepted_by_get_clozes_from_svg() {
    let svg = r#"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="400" height="200"><g id="markup-group"></g><g id="clozes-group"><g id="shape_group"><rect x="0" y="0" width="50" height="50"/><ellipse cx="100" cy="25" rx="20" ry="20"/></g><rect x="200" y="0" width="40" height="40"/></g></svg>"#;

    let mut svg_element = Element::parse(svg.as_bytes()).unwrap();
    let clozes = get_clozes_from_svg(&mut svg_element).unwrap();

    assert_eq!(clozes.len(), 2, "one group + one primitive = 2 clozes");
    assert_eq!(clozes[0].name, "g", "first cloze is the group");
    assert_eq!(clozes[1].name, "rect", "second cloze is the standalone rect");
}

/// `modify_clozes_for_card` with a group cloze (front, `ToAnswer + hint`) should:
/// - recursively apply the answer colour to every shape child
/// - wrap the group in a hint `<g>` with a `<text>` element whose position is
///   the centre of the group's bounding box
#[test]
fn test_grouped_shapes_modify_card_to_answer() {
    // Two shapes:
    //   rect  x=0  y=0  w=100 h=40  → bbox (0,  0, 100, 40)
    //   ellipse cx=200 cy=20 rx=20 ry=20 → bbox (180, 0, 220, 40)
    //   union bbox → (0, 0, 220, 40)  centre → (110, 20)
    let svg = r#"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="400" height="200"><g id="markup-group"></g><g id="clozes-group"><g id="group_1"><rect x="0" y="0" width="100" height="40"/><ellipse cx="200" cy="20" rx="20" ry="20"/></g></g></svg>"#;

    let mut svg_element = Element::parse(svg.as_bytes()).unwrap();
    let mut clozes = get_clozes_from_svg(&mut svg_element).unwrap();
    assert_eq!(clozes.len(), 1);

    let config = ImageOcclusionConfig::default();
    modify_clozes_for_card(
        &[(0, ClozeHiddenReplacement::ToAnswer { hint: Some("label".to_string()) })],
        &mut clozes,
        FrontConceal::OnlyGrouping,
        BackReveal::FullNote,
        false,
        CardSide::Front,
        &config,
    );

    // add_text_to_cloze wraps the group in a new <g> and adds a <text> sibling.
    let wrapper = &clozes[0];
    assert_eq!(wrapper.name, "g");
    assert_eq!(wrapper.children.len(), 2, "wrapper must have inner group + text");

    // ── inner group ──────────────────────────────────────────────────────────
    let inner = match &wrapper.children[0] {
        xmltree::XMLNode::Element(e) => e,
        other => panic!("expected Element, got {:?}", other),
    };
    assert_eq!(inner.name, "g");
    assert_eq!(inner.attributes.get("id").map(String::as_str), Some("group_1"));

    // Fill must have been applied recursively to every shape child, not just
    // inherited — explicit child fills override SVG inheritance.
    let shapes: Vec<&Element> = inner
        .children
        .iter()
        .filter_map(|n| match n {
            xmltree::XMLNode::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(shapes.len(), 2);
    assert!(
        shapes.iter().all(|s| s.attributes.get("fill").map(String::as_str) == Some("#FF7E7E")),
        "answer colour must be set on every child shape"
    );

    // ── hint text element ────────────────────────────────────────────────────
    let text_el = match &wrapper.children[1] {
        xmltree::XMLNode::Element(e) => e,
        other => panic!("expected Element, got {:?}", other),
    };
    assert_eq!(text_el.name, "text");
    assert_eq!(
        text_el.attributes.get("x").map(String::as_str),
        Some("110"),
        "text x must be at horizontal centre of union bounding box"
    );
    assert_eq!(
        text_el.attributes.get("y").map(String::as_str),
        Some("20"),
        "text y must be at vertical centre of union bounding box"
    );
    assert_eq!(
        text_el.children,
        vec![xmltree::XMLNode::Text("label".to_string())]
    );
}

/// When a group cloze is hidden (`hide_cloze_mask`), opacity is set on the
/// outer `<g>` which composites all children — no recursion needed.
#[test]
fn test_grouped_shapes_modify_card_hidden() {
    let svg = r#"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="400" height="200"><g id="markup-group"></g><g id="clozes-group"><g id="group_1"><rect x="0" y="0" width="100" height="40"/></g></g></svg>"#;

    let mut svg_element = Element::parse(svg.as_bytes()).unwrap();
    let mut clozes = get_clozes_from_svg(&mut svg_element).unwrap();

    let config = ImageOcclusionConfig::default();
    // Cloze 0 is not in cloze_indices → OnlyGrouping → hide it.
    modify_clozes_for_card(
        &[(999, ClozeHiddenReplacement::ToAnswer { hint: None })],
        &mut clozes,
        FrontConceal::OnlyGrouping,
        BackReveal::FullNote,
        false,
        CardSide::Front,
        &config,
    );

    assert_eq!(
        clozes[0].attributes.get("opacity").map(String::as_str),
        Some("0"),
        "hidden group must have opacity=0 set directly on the <g>"
    );
}

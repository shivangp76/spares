use crate::parsers::generate_files::CardSide;
use crate::parsers::{
    BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal, NotePart,
    Parseable, get_cards,
    image_occlusion::{
        ImageOcclusionConfig, ImageOcclusionData, get_clozes_from_svg,
        get_image_occlusion_directory, modify_clozes_for_card,
    },
    impls::markdown::MarkdownParser,
};
use indoc::indoc;
use pretty_assertions::assert_eq;
use std::{fs::read_to_string, path::PathBuf, sync::Arc};
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
        <!--- front_conceal = \"{:?}\" --->
        <!--- back_reveal = \"{:?}\" --->
        <!--- spares: image occlusion end --->
        b
        <!--- spares: image occlusion start --->
        <!--- original_image_filepath = \"{}\" --->
        <!--- clozes_filepath = \"{}\" --->
        <!--- front_conceal = \"{:?}\" --->
        <!--- back_reveal = \"{:?}\" --->
        [Image Occlusion](/some/random/image/path)
        <!--- spares: image occlusion end --->
        c" },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
        FrontConceal::OnlyGrouping,
        BackReveal::FullNote,
        original_image_filepath_2.display(),
        clozes_filepath_2.display(),
        FrontConceal::OnlyGrouping,
        BackReveal::FullNote,
    );

    let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let cards_res = get_cards(parser.as_ref(), None, note_data.as_str(), true, MOVE_FILES);
    assert!(cards_res.is_ok());
    let image_occlusion_1 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-1.svg"),
        clozes_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-1_clozes.svg"),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
    });
    let image_occlusion_2 = Arc::new(ImageOcclusionData {
        original_image_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-2.svg"),
        clozes_filepath: PathBuf::from("/tmp/spares/data/image_occlusions/test-A-2_clozes.svg"),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
    });
    if let Ok(cards) = cards_res {
        let expected = vec![
                CardData {
                    order: Some(1),
                    grouping: ClozeGrouping::Auto(1),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_type: BackType::FullNote,
                    data: vec![
                        NotePart::SurroundingData("a\n".to_string()),
                        NotePart::ImageOcclusion { cloze_indices: vec![(0, ClozeHiddenReplacement::ToAnswer{ hint: None })], data: image_occlusion_1.clone() },
                        NotePart::SurroundingData("b\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-2.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-2_clozes.svg\" --->\n<!--- front_conceal = \"OnlyGrouping\" --->\n<!--- back_reveal = \"FullNote\" --->\n![Test A 2](/tmp/spares/data/image_occlusions/test-A-2.svg)\n<!--- spares: image occlusion end --->\nc".to_string()),
                    ],
                },
                CardData {
                    order: Some(2),
                    grouping: ClozeGrouping::Auto(2),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_type: BackType::FullNote,
                    data: vec![
                        NotePart::SurroundingData("a\n".to_string()),
                        NotePart::ImageOcclusion { cloze_indices: vec![(1, ClozeHiddenReplacement::ToAnswer{ hint: None })], data: image_occlusion_1.clone() },
                        NotePart::SurroundingData("b\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-2.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-2_clozes.svg\" --->\n<!--- front_conceal = \"OnlyGrouping\" --->\n<!--- back_reveal = \"FullNote\" --->\n![Test A 2](/tmp/spares/data/image_occlusions/test-A-2.svg)\n<!--- spares: image occlusion end --->\nc".to_string()),
                    ],
                },
                CardData {
                    order: Some(3),
                    grouping: ClozeGrouping::Custom("1".to_string()),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_type: BackType::FullNote,
                    data: vec![
                        NotePart::SurroundingData("a\n<!--- spares: image occlusion start --->\n<!--- original_image_filepath = \"/tmp/spares/data/image_occlusions/test-A-1.svg\" --->\n<!--- clozes_filepath = \"/tmp/spares/data/image_occlusions/test-A-1_clozes.svg\" --->\n<!--- front_conceal = \"OnlyGrouping\" --->\n<!--- back_reveal = \"FullNote\" --->\n![Test A 1](/tmp/spares/data/image_occlusions/test-A-1.svg)\n<!--- spares: image occlusion end --->\nb\n".to_string()),
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
            <!--- front_conceal = \"{:?}\" --->
            <!--- back_reveal = \"{:?}\" --->
            <!--- spares: image occlusion end --->
            " },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
        FrontConceal::OnlyGrouping,
        BackReveal::FullNote,
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
    });
    let cards = cards_res.unwrap();
    let expected = vec![CardData {
        order: Some(1),
        grouping: ClozeGrouping::Custom("1".to_string()),
        is_suspended: Some(true),
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        back_type: BackType::FullNote,
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
                <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
              </g>
            </svg>"##
    };
    assert_eq!(card_cloze_data, expected_card_cloze_data);
}

#[test]
fn test_get_cards_image_occlusion_front_conceal() {
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
            <!--- front_conceal = \"{:?}\" --->
            <!--- back_reveal = \"{:?}\" --->
            <!--- spares: image occlusion end --->
            " },
        original_image_filepath_1.display(),
        clozes_filepath_1.display(),
        FrontConceal::AllGroupings,
        BackReveal::OnlyAnswered,
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
    });
    let cards = cards_res.unwrap();
    let expected = vec![
        CardData {
            order: Some(1),
            grouping: ClozeGrouping::Custom("1".to_string()),
            is_suspended: None,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_type: BackType::OnlyAnswered,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;o:1;f:all;b:a]".to_string()),
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
            grouping: ClozeGrouping::Auto(1),
            is_suspended: None,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_type: BackType::OnlyAnswered,
            data: vec![
                NotePart::SurroundingData("a".to_string()),
                NotePart::ClozeStart("{{[g:1;o:1;f:all;b:a]".to_string()),
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
                <ellipse cx="346.53" cy="78.95" fill="#FFEBA2" id="svg_3" rx="46.32" ry="46.32" stroke="#2D2D2D" stroke-dasharray="null" stroke-linecap="null" stroke-linejoin="null" data-cloze-settings="o:2" />
              </g>
            </svg>"##
    };
    assert_eq!(card_cloze_data, expected_card_cloze_data);
}

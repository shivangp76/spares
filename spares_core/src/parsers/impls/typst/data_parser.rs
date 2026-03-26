use crate::parsers::ClozeMatch;
use std::ops::Range;
use typst_syntax::{LinkedNode, SyntaxKind, parse};

const CLOZE_FUNC_NAME: &str = "cl";
const LINKED_NOTE_FUNC_NAME: &str = "lin";
const SETTINGS_FUNC_NAME: &str = "se";
const KEYWORD_FUNC_NAME: &str = "key";

/// A stateful iterator over parsed Typst source that yields clozes,
/// linked-note refs, settings blocks, and keyword blocks one at a time.
pub struct TypstDataParser {
    /// One entry per top-level `#cl[…]` call; each entry contains that call
    /// and all nested `#cl[…]` calls it contains, sorted start→end (outer first).
    pub clozes: Vec<Vec<ClozeMatch>>,
    pub linked_notes: Vec<Range<usize>>,
    pub settings: Vec<Range<usize>>,
    pub keywords: Vec<Range<usize>>,
}

impl TypstDataParser {
    pub fn new(input: &str) -> Self {
        let root = parse(input);
        let linked = LinkedNode::new(&root);

        let mut collector = Collector::default();
        collector.walk(&linked, 0);

        Self {
            clozes: collector.cloze_groups,
            linked_notes: collector.linked_notes,
            settings: collector.settings,
            keywords: collector.keywords,
        }
    }
}

#[derive(Default)]
struct Collector {
    /// Groups of clozes. Each entry is one top-level `#cl[…]` call together
    /// with all nested `#cl[…]` calls it contains, sorted start→end (so outer
    /// comes first, matching the original parser's contract).
    cloze_groups: Vec<Vec<ClozeMatch>>,
    linked_notes: Vec<Range<usize>>,
    settings: Vec<Range<usize>>,
    keywords: Vec<Range<usize>>,
    /// Nesting depth of `#cl[…]` calls currently being walked. Used to avoid
    /// treating nested `#cl` calls as new top-level cloze groups while still
    /// recursing into them to collect `#lin`, `#se`, and `#key` nodes.
    cloze_depth: usize,
}

impl Collector {
    /// Recursively walk the CST. `offset` is the byte position of `node`'s
    /// first byte in the original source string.
    fn walk(&mut self, node: &LinkedNode<'_>, offset: usize) {
        // We only care about FuncCall nodes — everything else we just descend.
        let mut entered_cloze = false;
        if node.kind() == SyntaxKind::FuncCall
            && let Some(name) = func_name(node, offset)
        {
            match name.text.as_str() {
                CLOZE_FUNC_NAME => {
                    if self.cloze_depth == 0 {
                        // Build a group for this top-level cloze call.
                        let group = collect_cloze_group(node, offset);
                        if !group.is_empty() {
                            self.cloze_groups.push(group);
                        }
                    }
                    // Track depth so nested `#cl` nodes aren't treated as new
                    // top-level groups, but we still recurse to collect
                    // `#lin`, `#se`, and `#key` nodes inside.
                    self.cloze_depth += 1;
                    entered_cloze = true;
                }
                LINKED_NOTE_FUNC_NAME => {
                    if let Some(range) = first_content_block_range(node, offset) {
                        self.linked_notes.push(range);
                    }
                }
                SETTINGS_FUNC_NAME => {
                    if let Some(range) = first_content_block_range(node, offset) {
                        self.settings.push(range);
                    }
                }
                KEYWORD_FUNC_NAME => {
                    if let Some(range) = first_content_block_range(node, offset) {
                        self.keywords.push(range);
                    }
                }
                _ => {}
            }
        }

        // Descend into children.
        let mut child_off = offset;
        for child in node.children() {
            self.walk(&child, child_off);
            child_off += child.len();
        }

        if entered_cloze {
            self.cloze_depth -= 1;
        }
    }
}

// ── Cloze group builder ───────────────────────────────────────────────────────

/// Build the flat, start-sorted `Vec<ClozeMatch>` for one top-level `#cl[…]`
/// call, including all nested `#cl[…]` calls inside it.
///
/// `#cl` supports two content-block arguments:
///
/// ```text
/// #cl[body]           → one arg
/// #cl[body][settings] → two args
/// ```
///
/// The CST represents this as a single `FuncCall` whose Args child may contain
/// two consecutive `ContentBlock` children:
///
/// ```text
/// FuncCall
/// ├── Ident "cl"
/// └── Args
///     ├── ContentBlock   ← first arg  (body)
///     └── ContentBlock   ← second arg (settings) — optional
/// ```
fn collect_cloze_group(call: &LinkedNode<'_>, call_offset: usize) -> Vec<ClozeMatch> {
    let mut out = Vec::new();
    collect_cloze_recursive(call, call_offset, &mut out);
    out.sort_by_key(|m| m.start_match.start);
    out
}

fn collect_cloze_recursive(call: &LinkedNode<'_>, call_offset: usize, out: &mut Vec<ClozeMatch>) {
    // `call` must be a FuncCall for "cl".
    let Some(cm) = build_cloze_match(call, call_offset) else {
        return;
    };
    out.push(cm);

    // Recurse into the body (first content block) to find nested cloze calls.
    let args_node = args_child(call, call_offset);
    let Some((args, args_off)) = args_node else {
        return;
    };

    let mut content_blocks_found = 0u32;
    let mut child_off = args_off;
    for child in args.children() {
        if child.kind() == SyntaxKind::ContentBlock && content_blocks_found == 0 {
            // This is the body block — descend into it for nested #cl calls.
            let mut body_child_off = child_off;
            for body_child in child.children() {
                find_nested_cloze_calls(&body_child, body_child_off, out);
                body_child_off += body_child.len();
            }
            content_blocks_found += 1;
        }
        child_off += child.len();
    }
}

/// Descend looking for `#cl[…]` calls anywhere inside `node`.
fn find_nested_cloze_calls(node: &LinkedNode<'_>, offset: usize, out: &mut Vec<ClozeMatch>) {
    if node.kind() == SyntaxKind::FuncCall
        && let Some(name) = func_name(node, offset)
        && name.text == CLOZE_FUNC_NAME
    {
        collect_cloze_recursive(node, offset, out);
        return; // collect_cloze_recursive handles recursion itself
    }
    let mut child_off = offset;
    for child in node.children() {
        find_nested_cloze_calls(&child, child_off, out);
        child_off += child.len();
    }
}

/// Build the `ClozeMatch` for a single `#cl[…]` `FuncCall` node.
///
/// `start_match` — range of the hash + ident + `[` (or `(`) opening the call,
///   i.e. the same bytes the original scanner consumed when it saw `#cl[`.
///
/// For a one-arg call `#cl[body]`:
///   `end_match`       = range of the closing `]`
///   `settings_match`  = empty (default Range)
///
/// For a two-arg call `#cl[body][settings]`:
///   `end_match`       = range from `]` of first arg through `]` of second arg
///                       (i.e. `][settings]` as one range, matching original)
///   `settings_match`  = content inside second `[…]` (exclusive of brackets)
///
/// Edge case — empty second arg `#cl[body][]`:
///   `settings_match` = default (empty) Range, matching original parser.
fn build_cloze_match(call: &LinkedNode<'_>, call_offset: usize) -> Option<ClozeMatch> {
    // ── locate Ident and Args ─────────────────────────────────────────────────
    let mut call_child_off = call_offset;
    let mut ident_range: Option<Range<usize>> = None;
    let mut args_range: Option<(LinkedNode<'_>, usize)> = None;

    for child in call.children() {
        let len = child.len();
        match child.kind() {
            SyntaxKind::Ident => {
                ident_range = Some(call_child_off..call_child_off + len);
            }
            SyntaxKind::Args => {
                args_range = Some((child.clone(), call_child_off));
            }
            _ => {}
        }
        call_child_off += len;
    }

    let ident_range = ident_range?;
    let (args_node, args_off) = args_range?;

    // ── find content blocks inside Args ──────────────────────────────────────
    // We expect 1 or 2 ContentBlock children. Anything else (e.g. a paren
    // argument list) is treated as having no content blocks.
    let mut blocks: Vec<(ContentBlockInfo, usize)> = Vec::new();
    let mut args_child_off = args_off;

    for child in args_node.children() {
        let len = child.len();
        if child.kind() == SyntaxKind::ContentBlock
            && let Some(info) = parse_content_block(&child, args_child_off)
        {
            blocks.push((info, args_child_off));
        }
        args_child_off += len;
    }

    if blocks.is_empty() {
        return None;
    }

    // `start_match` mirrors the original: `#cl[` (hash at call_offset, then
    // ident, then the `[` opening the first content block).
    // The original scanner set start = cursor_start (the `#`) ..
    // self.s.cursor() after eating `[`. That means it includes `#cl[`.
    // In Typst's CST the `#` sigil is a sibling node immediately before the
    // FuncCall, so `call_offset` points to the ident (`c` in `cl`). Subtract
    // 1 to recover the `#` position and match the original scanner's contract.
    let start_match = call_offset.saturating_sub(1)..blocks[0].0.open_bracket.end;

    // ── single-arg form ───────────────────────────────────────────────────────
    if blocks.len() == 1 {
        let close = &blocks[0].0.close_bracket;
        return Some(ClozeMatch {
            start_match,
            end_match: close.clone(),
            settings_match: Range::default(),
        });
    }

    // ── two-arg form ──────────────────────────────────────────────────────────
    // end_match  = first-block's `]` start .. second-block's `]` end
    //            = the span `][settings]` (inclusive of both brackets).
    let first_close = &blocks[0].0.close_bracket;
    let second_open = &blocks[1].0.open_bracket;
    let second_close = &blocks[1].0.close_bracket;
    let second_content = &blocks[1].0.content_range;

    let end_match = first_close.start..second_close.end;
    let settings_match = if second_content.is_empty() {
        Range::default()
    } else {
        second_content.clone()
    };

    Some(ClozeMatch {
        start_match,
        end_match,
        settings_match,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

struct IdentInfo {
    text: String,
}

struct ContentBlockInfo {
    open_bracket: Range<usize>,
    close_bracket: Range<usize>,
    content_range: Range<usize>,
}

/// Return the function name from a `FuncCall` node (the Ident child).
fn func_name(call: &LinkedNode<'_>, call_offset: usize) -> Option<IdentInfo> {
    let mut child_off = call_offset;
    for child in call.children() {
        let len = child.len();
        if child.kind() == SyntaxKind::Ident {
            return Some(IdentInfo {
                text: child.text().to_string(),
            });
        }
        child_off += len;
    }
    None
}

/// Return the (Args child, its byte offset) for a `FuncCall` node.
fn args_child<'a>(call: &'a LinkedNode<'a>, call_offset: usize) -> Option<(LinkedNode<'a>, usize)> {
    let mut child_off = call_offset;
    for child in call.children() {
        let len = child.len();
        if child.kind() == SyntaxKind::Args {
            return Some((child, child_off));
        }
        child_off += len;
    }
    None
}

/// Parse open/close bracket positions and content range from a `ContentBlock`.
fn parse_content_block(block: &LinkedNode<'_>, block_offset: usize) -> Option<ContentBlockInfo> {
    let mut open: Option<Range<usize>> = None;
    let mut close: Option<Range<usize>> = None;
    let mut child_off = block_offset;

    for child in block.children() {
        let len = child.len();
        match child.kind() {
            SyntaxKind::LeftBracket => open = Some(child_off..child_off + len),
            SyntaxKind::RightBracket => close = Some(child_off..child_off + len),
            _ => {}
        }
        child_off += len;
    }

    let open = open?;
    let close = close?;
    let content_range = open.end..close.start;

    Some(ContentBlockInfo {
        open_bracket: open,
        close_bracket: close,
        content_range,
    })
}

/// Return the content range (inside `[…]`) of the *first* `ContentBlock` arg of
/// a function call. Used for `#lin`, `#se`, `#key`.
fn first_content_block_range(call: &LinkedNode<'_>, call_offset: usize) -> Option<Range<usize>> {
    let (args, args_off) = args_child(call, call_offset)?;
    let mut child_off = args_off;
    for child in args.children() {
        let len = child.len();
        if child.kind() == SyntaxKind::ContentBlock {
            let info = parse_content_block(&child, child_off)?;
            return Some(info.content_range);
        }
        child_off += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::ops::Range;

    #[test]
    fn test_basic_setting() {
        let input = "Test #se[basic] asd";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.settings, vec![9..14]);
    }

    #[test]
    fn test_basic_linked_note() {
        let input = "Test #lin[basic] #test[p]asd";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.linked_notes, vec![10..15]);
    }

    #[test]
    fn test_advanced_linked_note() {
        let input = "Test #lin[basic [a] b] #test[p]asd";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.linked_notes, vec![10..21]);
    }

    #[test]
    fn test_linked_note_in_math() {
        let input = "Test $ a &= b #[(bc of #lin[Rule A])] \\ b &=c $";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.linked_notes, vec![28..34]);
    }

    #[test]
    fn test_basic_keyword() {
        let input = "Test #key[basic] #test[p]asd";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.keywords, vec![10..15]);
    }

    #[test]
    fn test_setting_with_cloze() {
        let input = "Test #se[basic] asd #cl[test] #se[key]";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.settings, vec![9..14, 34..37]);
    }

    #[test]
    fn test_basic_cloze() {
        let input = "Test #cl[basic] asd";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 14..15,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_empty_settings() {
        let input = "Test #cl[basic][] cloze";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 14..17,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_escaped_character() {
        let input = "Test \\#cl[basic] cloze";
        let parser = TypstDataParser::new(input);
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(parser.clozes, expected);
    }

    #[test]
    fn test_commented() {
        let input = "// Test #cl[basic] cloze\n #cl[b]";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 26..30,
                end_match: 31..32,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_unmatched_open() {
        let input = "Test #cl[ test";
        let parser = TypstDataParser::new(input);
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(parser.clozes, expected);
    }

    #[test]
    fn test_math_mode() {
        let input = "test #cl[$\n( b]\n\n$][g:1] test";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 18..24,
                settings_match: 20..23,
            }]]
        );
    }

    #[test]
    fn test_code_mode() {
        let input = "test #cl[#(let a = 2)][g:1] test";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 21..27,
                settings_match: 23..26,
            }]]
        );
    }

    #[test]
    fn test_cloze_in_math_1() {
        let input = "Test $ a &= b #[(bc of #cl[Rule A])] \\ b &=c #cl[a][g:1] $";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![
                vec![ClozeMatch {
                    start_match: 23..27,
                    end_match: 33..34,
                    settings_match: Range::default(),
                }],
                vec![ClozeMatch {
                    start_match: 45..49,
                    end_match: 50..56,
                    settings_match: 52..55
                }],
            ]
        );
    }

    #[test]
    fn test_cloze_in_math_2_with_settings() {
        let input = "Test $ a &= b #[(bc of #cl[Rule A][g:[1]])] \\ b &=c #cl[a][g:1] $";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![
                vec![ClozeMatch {
                    start_match: 23..27,
                    end_match: 33..41,
                    settings_match: 35..40,
                }],
                vec![ClozeMatch {
                    start_match: 52..56,
                    end_match: 57..63,
                    settings_match: 59..62
                }],
            ]
        );
    }

    #[test]
    fn test_cloze_in_math_with_code() {
        let input = "#cl[ $ #table() $ ]";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 18..19,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_cloze_in_math_with_code_2() {
        let input = "#cl[ $ #let a = b \\ $ ]";
        let parser = TypstDataParser::new(input);
        assert_eq!(parser.clozes.len(), 1);
    }

    #[test]
    fn test_cloze_in_command() {
        let input = "#strong[bc of #cl[Rule A])]";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 14..18,
                end_match: 24..25,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_nested_cloze() {
        let input = "test #cl[#cl[b][g:1]#cl[$a s (d)$]][g:1] test";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![
                ClozeMatch {
                    start_match: 5..9,
                    end_match: 34..40,
                    settings_match: 36..39,
                },
                ClozeMatch {
                    start_match: 9..13,
                    end_match: 14..20,
                    settings_match: 16..19,
                },
                ClozeMatch {
                    start_match: 20..24,
                    end_match: 33..34,
                    settings_match: Range::default(),
                }
            ]]
        );
    }

    #[test]
    fn test_empty_cloze() {
        let input = "Test #cl[] end";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 9..10,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_other_func_in_cloze_1() {
        let input = indoc! { r#"#cl[ - mnemonic: #strong[c]url = #strong[c]ross product ]"# };
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 56..57,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_other_func_in_cloze_2() {
        let input = indoc! { r#"#cl[ #lin[upper envelope] test ][o:1] "# };
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 31..37,
                settings_match: 33..36,
            }]]
        );
    }

    #[test]
    fn test_comment_in_code_mode_within_cloze() {
        let input = "#cl[test ```py\n m.sample() # equal probability\n  ```\n]";
        let parser = TypstDataParser::new(input);
        assert_eq!(
            parser.clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 53..54,
                settings_match: Range::default(),
            }]]
        );
    }
}

//! Splitting an assistant reply's markdown into renderable blocks.
//!
//! Slint's `StyledText` covers only a subset of CommonMark: emphasis,
//! strikethrough, inline code, links and lists. Every other construct
//! (headings, fenced code blocks, tables, block quotes, rules, footnotes,
//! images, raw HTML, math) is reported as an error by
//! `StyledText::from_markdown`, and that call returns `Err` as soon as *one*
//! error is recorded (`i-slint-core-1.17.1/styled_text.rs:53-62`) - so a
//! single fenced code block used to drop the whole message back to plain
//! text.
//!
//! Splitting the source into top-level blocks first turns that all-or-nothing
//! fallback into a per-block one: paragraphs and lists still render styled,
//! and the constructs `StyledText` can't take are rendered by the UI itself
//! (a code block gets a monospace, selectable `CopiableText` on a filled
//! card, a heading gets a bold label, a rule gets a divider). It also buys
//! back text selection where it matters most: `StyledText` has no selection
//! of any kind - the element exposes no cursor, anchor or selection property
//! at all (`i-slint-compiler-1.17.1/builtins.slint:731-753`), and that is
//! deliberate upstream (slint-ui/slint#9560) - whereas the blocks rendered
//! through `CopiableText` are `TextInput`s and select on click-drag as usual.
//!
//! The classification is not a hand-maintained list of what `StyledText`
//! supports; every candidate block is *actually* handed to
//! `StyledText::from_markdown` and falls back to plain text if it returns
//! `Err`. So this stays correct if Slint's supported subset grows.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use slint::StyledText;

/// How the UI should render one block. Mirrors the `kind` string on
/// `MessageBlockData` in `app-window.slint` - keep the two in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// Renders through `StyledText` (markdown parsed successfully).
    Styled,
    /// A fenced or indented code block: monospace, selectable, with its own
    /// copy action.
    Code,
    /// An ATX/setext heading, rendered as a bold label.
    Heading,
    /// Anything `StyledText` rejected: a table, a block quote, a paragraph
    /// holding an image or raw HTML. Rendered as selectable plain text,
    /// showing the markdown source as-is. (A lone unbalanced "**" mid-stream
    /// is *not* one of these - CommonMark makes it literal text, so it still
    /// renders styled.)
    Text,
    /// A thematic break.
    Rule,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Styled => "styled",
            BlockKind::Code => "code",
            BlockKind::Heading => "heading",
            BlockKind::Text => "text",
            BlockKind::Rule => "rule",
        }
    }
}

/// One top-level block of an assistant reply.
#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    /// What a non-`Styled` block displays: the code for `Code`, the heading
    /// text for `Heading`, the raw markdown source for `Text`.
    pub content: String,
    /// The parsed rendering, `None` unless `kind == Styled`.
    pub styled: Option<StyledText>,
    /// This block's text with all markdown markers removed - what "Copier"
    /// puts on the clipboard, joined across blocks by [`plain_text`].
    pub plain: String,
    /// A fenced code block's info string ("rust", "bash", ...), else empty.
    pub language: String,
    /// 1-6 for `Heading`, 0 otherwise.
    pub heading_level: i32,
    /// One top-level item of a bullet/numbered list, split out of its list so
    /// the UI can put a real gap between items (a whole list is one
    /// `StyledText`, which stacks its item paragraphs with no leading at all -
    /// there is no line-height knob, verified). Renders exactly like the block
    /// it would otherwise be part of - `Styled` normally, `Text` if the item
    /// holds something `StyledText` rejects - and still carries its own `- ` /
    /// `1. ` marker, so nesting and ordered numbering are preserved. The UI
    /// tightens the gap between two consecutive `list_item` blocks back down
    /// relative to the gap between unrelated blocks; [`plain_text`] joins them
    /// with a single newline so a copied list stays a list.
    pub list_item: bool,
}

/// The parse options block splitting runs under.
///
/// `ENABLE_STRIKETHROUGH` matches what `i-slint-common` uses for
/// `StyledText`, so a `~~x~~` isn't split differently from how it renders.
/// `ENABLE_TABLES` is on top of that on purpose: a table is not something
/// `StyledText` accepts, and recognizing it here keeps it in one block
/// instead of letting each of its rows become a separate paragraph.
fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES
}

/// Split an assistant reply into top-level blocks.
///
/// Returns an empty vec for empty input. Callers treat "no blocks" as "render
/// this message as one plain `CopiableText`", which is also what happens for
/// user and system messages (never markdown) - see `ChatMessageRow`.
pub fn split_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    // The top-level element currently open: its tag and where it starts in
    // the source. `into_offset_iter` gives a `Start` event the range of the
    // *whole* element, but taking the end from the matching `End` event keeps
    // this correct regardless.
    let mut open: Option<(Tag, usize)> = None;

    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    open = Some((tag, range.start));
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some((tag, start)) = open.take() {
                        let source = markdown[start..range.end].trim_end();
                        push_top_level(&mut blocks, tag, source);
                    }
                }
            }
            // Standalone block-level events, which have no Start/End pair.
            Event::Rule if depth == 0 => blocks.push(Block {
                kind: BlockKind::Rule,
                content: String::new(),
                styled: None,
                plain: String::new(),
                language: String::new(),
                heading_level: 0,
                list_item: false,
            }),
            Event::Html(_) if depth == 0 => blocks.push(plain_block(markdown[range].trim_end())),
            _ => {}
        }
    }

    // An unterminated element (the common case while streaming: a fence
    // opened but not yet closed) never emits its `End`, so flush what's open.
    if let Some((tag, start)) = open {
        push_top_level(&mut blocks, tag, markdown[start..].trim_end());
    }

    blocks
}

/// Append the block(s) for one top-level element. Everything is one block
/// except a list, which is split into one block per top-level item (see
/// [`Block::list_item`]).
fn push_top_level(blocks: &mut Vec<Block>, tag: Tag, source: &str) {
    match tag {
        Tag::List(_) => blocks.extend(list_item_blocks(source)),
        _ => blocks.push(block_from(tag, source)),
    }
}

/// Split a list's source into one block per top-level item, marker included.
///
/// Nested lists stay inside their parent item's slice, so they render nested
/// and copy indented as before; an ordered list's start number rides along in
/// each item's `N. ` marker, so `StyledText` re-derives the right sequence.
fn list_item_blocks(list_source: &str) -> Vec<Block> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut item_start: Option<usize> = None;

    for (event, range) in Parser::new_ext(list_source, options()).into_offset_iter() {
        match event {
            // depth 1 == directly inside the list this fn was handed.
            Event::Start(Tag::Item) if depth == 1 => {
                item_start = Some(range.start);
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::Item) if depth == 2 => {
                depth -= 1;
                if let Some(start) = item_start.take() {
                    items.push(list_item_block(list_source[start..range.end].trim_end()));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    // Streaming: the last item's `End` hasn't arrived yet.
    if let Some(start) = item_start {
        items.push(list_item_block(list_source[start..].trim_end()));
    }
    // Nothing recognisable as an item (shouldn't happen for a real list) -
    // keep the whole thing as one block rather than dropping it.
    if items.is_empty() {
        items.push(block_from(Tag::List(None), list_source));
    }
    items
}

/// One list item as its own block: styled like any paragraph unless it holds
/// something `StyledText` rejects, and flagged so the UI and [`plain_text`]
/// treat a run of them as one list.
fn list_item_block(source: &str) -> Block {
    let mut block = match StyledText::from_markdown(source) {
        Ok(styled) => Block {
            kind: BlockKind::Styled,
            content: source.to_string(),
            styled: Some(styled),
            plain: strip_markup(source),
            language: String::new(),
            heading_level: 0,
            list_item: false,
        },
        Err(_) => plain_block(source),
    };
    block.list_item = true;
    block
}

/// Everything the "Copier" action puts on the clipboard: the message with
/// markdown markers stripped, i.e. as close as possible to what is on screen.
pub fn plain_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    let mut prev_list_item = false;
    for block in blocks {
        if block.kind == BlockKind::Rule {
            continue;
        }
        if !out.is_empty() {
            // A run of split-out list items is one list on screen, so keep it
            // one list on the clipboard: a single newline between them, a
            // blank line everywhere else.
            out.push_str(if prev_list_item && block.list_item {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(&block.plain);
        prev_list_item = block.list_item;
    }
    out
}

/// Classify one top-level element and build its `Block`.
fn block_from(tag: Tag, source: &str) -> Block {
    match tag {
        Tag::Heading { level, .. } => Block {
            kind: BlockKind::Heading,
            content: strip_markup(source),
            styled: None,
            plain: strip_markup(source),
            language: String::new(),
            heading_level: level as i32,
            list_item: false,
        },
        Tag::CodeBlock(kind) => {
            let language = match kind {
                // The info string can carry more than the language
                // ("rust,ignore"); only the first word names it.
                CodeBlockKind::Fenced(info) => {
                    info.split(',').next().unwrap_or("").trim().to_string()
                }
                CodeBlockKind::Indented => String::new(),
            };
            let code = code_text(source);
            Block {
                kind: BlockKind::Code,
                content: code.clone(),
                styled: None,
                plain: code,
                language,
                heading_level: 0,
                list_item: false,
            }
        }
        // Paragraphs and lists are the two things `StyledText` renders, but
        // ask it rather than assume: a paragraph holding an image, raw HTML
        // or - mid-stream - unbalanced markup is rejected too.
        Tag::Paragraph | Tag::List(_) => match StyledText::from_markdown(source) {
            Ok(styled) => Block {
                kind: BlockKind::Styled,
                content: source.to_string(),
                styled: Some(styled),
                plain: strip_markup(source),
                language: String::new(),
                heading_level: 0,
                list_item: false,
            },
            Err(_) => plain_block(source),
        },
        // Tables, block quotes, footnote definitions, definition lists: shown
        // as their markdown source, which stays readable and selectable.
        _ => plain_block(source),
    }
}

fn plain_block(source: &str) -> Block {
    Block {
        kind: BlockKind::Text,
        content: source.to_string(),
        styled: None,
        plain: source.to_string(),
        language: String::new(),
        heading_level: 0,
        list_item: false,
    }
}

/// The text of a code block, without its fences or indentation.
fn code_text(source: &str) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(source, options()) {
        if let Event::Text(text) = event {
            out.push_str(&text);
        }
    }
    // A fenced block's last text event keeps the newline before the closing
    // fence; nothing downstream wants a trailing blank line.
    out.trim_end_matches('\n').to_string()
}

/// One block's text with markdown markers removed.
///
/// Line breaks follow what `StyledText` actually draws rather than CommonMark
/// HTML semantics: it turns a soft break into a new paragraph
/// (`i-slint-common-1.17.1/styled_text.rs:343`), i.e. a visible line break,
/// so a copy of the rendered text has to break there too.
fn strip_markup(source: &str) -> String {
    let mut out = String::new();
    // One entry per open list; `Some(n)` is the next number of an ordered
    // list, `None` marks a bullet list.
    let mut lists: Vec<Option<u64>> = Vec::new();

    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Start(Tag::List(start)) => lists.push(start),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&"  ".repeat(lists.len().saturating_sub(1)));
                match lists.last_mut() {
                    Some(Some(number)) => {
                        out.push_str(&format!("{number}. "));
                        *number += 1;
                    }
                    _ => out.push_str("- "),
                }
            }
            Event::End(TagEnd::Item | TagEnd::Paragraph | TagEnd::Heading(_)) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            _ => {}
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(markdown: &str) -> Vec<BlockKind> {
        split_blocks(markdown).iter().map(|b| b.kind).collect()
    }

    #[test]
    fn paragraph_renders_styled() {
        assert_eq!(kinds("Voici un **plan**."), vec![BlockKind::Styled]);
    }

    #[test]
    fn code_block_no_longer_drops_the_whole_message() {
        // The regression this module exists for: `from_markdown` on the whole
        // string returns Err, so before splitting, all three blocks rendered
        // as raw markdown source.
        let markdown = "Avant.\n\n```rust\nlet x = 1;\n```\n\nAprès.";
        assert!(StyledText::from_markdown(markdown).is_err());
        assert_eq!(
            kinds(markdown),
            vec![BlockKind::Styled, BlockKind::Code, BlockKind::Styled]
        );
        let blocks = split_blocks(markdown);
        assert_eq!(blocks[1].content, "let x = 1;");
        assert_eq!(blocks[1].language, "rust");
    }

    #[test]
    fn headings_rules_and_tables_get_their_own_block() {
        let blocks = split_blocks("# Titre\n\nTexte.\n\n---\n\n| a | b |\n| - | - |\n| 1 | 2 |\n");
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![
                BlockKind::Heading,
                BlockKind::Styled,
                BlockKind::Rule,
                BlockKind::Text,
            ]
        );
        assert_eq!(blocks[0].content, "Titre");
        assert_eq!(blocks[0].heading_level, 1);
        assert!(blocks[3].content.starts_with("| a | b |"));
    }

    #[test]
    fn list_items_become_separate_blocks_keeping_nesting() {
        // One block per *top-level* item so the UI can space them apart; a
        // nested list stays inside its parent item's block.
        let blocks = split_blocks("- un\n- deux\n  - imbriqué\n");
        assert_eq!(blocks.len(), 2);
        assert!(blocks
            .iter()
            .all(|b| b.kind == BlockKind::Styled && b.list_item));
        assert_eq!(blocks[0].plain, "- un");
        assert_eq!(blocks[1].plain, "- deux\n  - imbriqué");
    }

    #[test]
    fn ordered_list_items_keep_their_number_when_split() {
        let blocks = split_blocks("1. a.pdf\n2. b.pdf\n3. c.pdf\n");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1].plain, "2. b.pdf");
        assert_eq!(blocks[2].plain, "3. c.pdf");
        assert!(blocks.iter().all(|b| b.list_item));
    }

    #[test]
    fn unterminated_fence_still_renders_as_code() {
        // What every streamed code block looks like until its closing fence
        // arrives.
        let blocks = split_blocks("```\nlet x = 1;");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Code);
        assert_eq!(blocks[0].content, "let x = 1;");
    }

    #[test]
    fn unsupported_inline_markup_falls_back_for_that_block_only() {
        // An image and raw HTML are the two things a paragraph can hold that
        // `StyledText` refuses (a lone unbalanced "**" is *not* one of them -
        // CommonMark makes it literal text, and it renders styled).
        let blocks = split_blocks("Fait.\n\n![capture](a.png)\n\n<b>gras</b>");
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![BlockKind::Styled, BlockKind::Text, BlockKind::Text]
        );
        assert_eq!(blocks[1].content, "![capture](a.png)");
    }

    #[test]
    fn block_quote_keeps_the_rest_of_the_message_styled() {
        let blocks = split_blocks("Avant.\n\n> cité\n\nAprès.");
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![BlockKind::Styled, BlockKind::Text, BlockKind::Styled]
        );
    }

    #[test]
    fn plain_text_strips_markers_and_joins_blocks() {
        let blocks = split_blocks(
            "## Résumé\n\nJ'ai **déplacé** 3 fichiers vers `~/Documents` :\n\n1. a.pdf\n2. b.pdf\n",
        );
        assert_eq!(
            plain_text(&blocks),
            "Résumé\n\nJ'ai déplacé 3 fichiers vers ~/Documents :\n\n1. a.pdf\n2. b.pdf"
        );
    }

    #[test]
    fn soft_break_becomes_a_line_break() {
        let blocks = split_blocks("une ligne\net la suite");
        assert_eq!(blocks[0].plain, "une ligne\net la suite");
    }
}

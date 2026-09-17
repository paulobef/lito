//! Best-effort syntax highlighting for fenced code blocks and the terminal
//! commands `RUN_COMMAND` steps run.
//!
//! Slint's text elements (`Text`, `TextInput` - what `CopiableText` wraps)
//! expose a single, uniform `color` property; there is no way to color
//! individual runs inside one of them, and `StyledText` (the other text
//! primitive) only carries the small set of styles CommonMark markdown
//! defines (emphasis/strong/strikethrough/links/code spans) - verified on
//! this repo's pinned Slint 1.17.1 that `<span style="color:...">` in its
//! `from_markdown` is *not* actually wired up despite being documented
//! upstream (`StyledText::from_markdown` returns an "HTML tag <span> is not
//! supported" error for it).
//!
//! So real per-token color means splitting a line into one small `CodeSpan`
//! (a single-line `TextInput`, see `app-window.slint`) per token instead of
//! one text element per line - this module only produces the *classification*
//! (`highlight_code`/`highlight_shell_command`/`classify_shell_word`), kept
//! deliberately dumb (no real parser, no per-language grammar file): a chat
//! reply's code blocks are short, and a hand-rolled scanner covering the
//! languages Lito actually sees (shell, and whatever a local model writes:
//! Rust, Python, JS/TS, JSON/TOML/YAML, C-like) is a fraction of the size and
//! start-up cost of pulling in a full grammar-based highlighter, at the cost
//! of missing edge cases (nested string interpolation, raw strings with
//! custom delimiters, ...) a real one would get right. Good enough for
//! "this reads as code, not a wall of one-color text", not a replacement for
//! an editor.
//!
//! Mirrors `markdown::BlockKind` in spirit: the classification is data, and
//! the caller (`view_model.rs`) is the only place that turns it into Slint
//! model structs, exactly like `Block`/`MessageBlockData`.

/// What a token in a highlighted line renders as. Mirrors the `class` string
/// on `CodeSpanData` in `app-window.slint` (`Syntax.color`/`.italic`/
/// `.weight` there switch on `as_str()`'s values) - keep the two in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    /// No special role - rendered in whatever color the surrounding context
    /// uses for plain text (a code block's body text vs. a command line's
    /// muted one), which is why it's the one class `Syntax.color` takes an
    /// explicit fallback color for rather than hardcoding.
    Plain,
    /// A language keyword (`let`, `if`, `import`, ...) or, for a shell
    /// command, a `-x`/`--long` flag - both read as "a fixed word from the
    /// language/tool's own vocabulary", so they share a color rather than
    /// adding a fifth token-class hue.
    Keyword,
    /// A quoted string literal, or an argv element containing whitespace
    /// (which would need quoting to be typed back in verbatim).
    String,
    /// A line or block comment. Rendered in `Palette.text-faint`, italic -
    /// the same "de-emphasized" treatment used for faint chrome text
    /// elsewhere, not a dedicated hue, since a comment is meant to recede.
    Comment,
    /// A numeric literal.
    Number,
    /// A command/function name: the first word of a shell command line, or
    /// (best-effort, no scope tracking) a name immediately followed by `(`.
    Function,
}

impl TokenClass {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenClass::Plain => "plain",
            TokenClass::Keyword => "keyword",
            TokenClass::String => "string",
            TokenClass::Comment => "comment",
            TokenClass::Number => "number",
            TokenClass::Function => "function",
        }
    }
}

/// One colored run of text. Never spans a line break - a multi-line
/// construct (a block comment, a triple-quoted string) is cut into one span
/// per physical line so the Rust side can hand each source line its own row
/// of spans without re-deriving line boundaries in `.slint`.
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub class: TokenClass,
}

/// A per-language scanner profile. Deliberately small: comment markers, a
/// keyword list, and whether `$name`/`${name}` shell variables and
/// leading `-`/`--` flags apply (only shell-family languages have either).
struct Lang {
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    keywords: &'static [&'static str],
    shell_like: bool,
}

const BASH_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "return", "export", "local", "readonly", "in", "select", "break", "continue",
    "exit", "shift", "trap", "set", "unset", "declare", "source",
];

const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "struct", "enum", "impl", "trait", "use", "mod", "match", "if",
    "else", "for", "while", "loop", "return", "break", "continue", "self", "Self", "super",
    "crate", "const", "static", "async", "await", "move", "ref", "dyn", "where", "as", "in",
    "unsafe", "extern", "type", "true", "false", "None", "Some", "Ok", "Err",
];

const PYTHON_KEYWORDS: &[&str] = &[
    "def", "class", "import", "from", "as", "return", "if", "elif", "else", "for", "while", "with",
    "try", "except", "finally", "raise", "yield", "lambda", "pass", "break", "continue", "in",
    "is", "not", "and", "or", "None", "True", "False", "self", "global", "nonlocal", "async",
    "await", "del", "assert",
];

const JS_KEYWORDS: &[&str] = &[
    "function",
    "const",
    "let",
    "var",
    "return",
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "class",
    "extends",
    "new",
    "this",
    "import",
    "from",
    "export",
    "default",
    "try",
    "catch",
    "finally",
    "throw",
    "async",
    "await",
    "typeof",
    "instanceof",
    "in",
    "of",
    "null",
    "undefined",
    "true",
    "false",
    "interface",
    "type",
    "implements",
    "public",
    "private",
    "protected",
    "static",
    "readonly",
    "enum",
];

const C_LIKE_KEYWORDS: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "return",
    "struct",
    "class",
    "enum",
    "public",
    "private",
    "protected",
    "static",
    "const",
    "void",
    "int",
    "float",
    "double",
    "char",
    "bool",
    "true",
    "false",
    "null",
    "nullptr",
    "new",
    "delete",
    "import",
    "package",
    "func",
    "var",
    "let",
    "interface",
    "extends",
    "implements",
];

fn lang_for(language: &str) -> Lang {
    match language.trim().to_ascii_lowercase().as_str() {
        "bash" | "sh" | "shell" | "zsh" | "console" => Lang {
            line_comment: &["#"],
            block_comment: None,
            keywords: BASH_KEYWORDS,
            shell_like: true,
        },
        "rust" | "rs" => Lang {
            line_comment: &["//"],
            block_comment: Some(("/*", "*/")),
            keywords: RUST_KEYWORDS,
            shell_like: false,
        },
        "python" | "py" => Lang {
            line_comment: &["#"],
            block_comment: None,
            keywords: PYTHON_KEYWORDS,
            shell_like: false,
        },
        "javascript" | "js" | "jsx" | "typescript" | "ts" | "tsx" | "mjs" | "cjs" => Lang {
            line_comment: &["//"],
            block_comment: Some(("/*", "*/")),
            keywords: JS_KEYWORDS,
            shell_like: false,
        },
        "json" | "json5" | "jsonc" => Lang {
            line_comment: &[],
            block_comment: None,
            keywords: &["true", "false", "null"],
            shell_like: false,
        },
        "toml" => Lang {
            line_comment: &["#"],
            block_comment: None,
            keywords: &["true", "false"],
            shell_like: false,
        },
        "yaml" | "yml" => Lang {
            line_comment: &["#"],
            block_comment: None,
            keywords: &["true", "false", "null"],
            shell_like: false,
        },
        "c" | "cpp" | "c++" | "h" | "hpp" | "java" | "go" | "swift" | "kotlin" | "kt" | "cs" => {
            Lang {
                line_comment: &["//"],
                block_comment: Some(("/*", "*/")),
                keywords: C_LIKE_KEYWORDS,
                shell_like: false,
            }
        }
        // Unknown/unlabeled fence: still worth picking out strings, numbers
        // and the two most common comment markers rather than leaving the
        // whole block one flat color.
        _ => Lang {
            line_comment: &["#", "//"],
            block_comment: None,
            keywords: &[],
            shell_like: false,
        },
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Highlight a fenced code block's source, one row of spans per source line.
/// `language` is the fence's info string (`markdown::Block::language`, e.g.
/// `"rust"`), matched case-insensitively; an unrecognized or empty language
/// falls back to a generic scan (strings/numbers/comments only).
pub fn highlight_code(code: &str, language: &str) -> Vec<Vec<Span>> {
    let lang = lang_for(language);
    let mut in_block_comment = false;
    code.lines()
        .map(|line| highlight_line(line, &lang, &mut in_block_comment, true))
        .collect()
}

/// Highlight a single already-assembled shell command line (e.g. the plan
/// panel's `"$ " + step.command`) - the first word is always treated as the
/// program name (`Function`), `-x`/`--long` flags and shell keywords as
/// `Keyword`, `$VAR`/`${VAR}` as `Function`, quoted strings as `String`.
pub fn highlight_shell_command(command: &str) -> Vec<Span> {
    let lang = lang_for("bash");
    let mut in_block_comment = false;
    highlight_line(command, &lang, &mut in_block_comment, true)
}

/// Classify one already-split argv element (an `ApprovalCard` row): unlike
/// [`highlight_shell_command`], the word is never re-split on whitespace -
/// an argv element containing spaces is one OS-level argument, not several
/// shell words, so splitting it would misrepresent what actually runs.
pub fn classify_shell_word(word: &str, is_program: bool) -> TokenClass {
    if is_program {
        return TokenClass::Function;
    }
    if word.starts_with('$') && word.len() > 1 {
        return TokenClass::Function;
    }
    if word.len() > 1 && word.starts_with('-') && !word[1..].starts_with(char::is_numeric) {
        return TokenClass::Keyword;
    }
    if !word.is_empty() && looks_numeric(word) {
        return TokenClass::Number;
    }
    if word.is_empty() || word.chars().any(char::is_whitespace) {
        return TokenClass::String;
    }
    TokenClass::Plain
}

fn looks_numeric(word: &str) -> bool {
    let body = word.strip_prefix('-').unwrap_or(word);
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Scan one physical line. `in_block_comment` carries state across lines for
/// multi-line `/* ... */` comments; `first_word_is_function` marks a shell
/// command's leading word (the program name) as `Function` regardless of
/// whether it's also a keyword.
fn highlight_line(
    line: &str,
    lang: &Lang,
    in_block_comment: &mut bool,
    first_word_is_function: bool,
) -> Vec<Span> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0usize;
    let mut seen_word = false;

    let push = |spans: &mut Vec<Span>, text: String, class: TokenClass| {
        if text.is_empty() {
            return;
        }
        // Merge into the previous span when it's the same class - mainly
        // collapses runs of plain punctuation/whitespace into one `Text`
        // element instead of one per character.
        if let Some(last) = spans.last_mut() {
            if last.class == class {
                last.text.push_str(&text);
                return;
            }
        }
        spans.push(Span { text, class });
    };

    while i < chars.len() {
        // Continuing a block comment opened on an earlier line.
        if *in_block_comment {
            if let Some((_, close)) = lang.block_comment {
                if let Some(end) = find_str(&chars, i, close) {
                    let text: String = chars[i..end + close.chars().count()].iter().collect();
                    push(&mut spans, text, TokenClass::Comment);
                    i = end + close.chars().count();
                    *in_block_comment = false;
                    continue;
                }
            }
            let text: String = chars[i..].iter().collect();
            push(&mut spans, text, TokenClass::Comment);
            break;
        }

        let c = chars[i];

        // Block comment open.
        if let Some((open, close)) = lang.block_comment {
            if starts_with_at(&chars, i, open) {
                if let Some(end) = find_str(&chars, i + open.chars().count(), close) {
                    let text: String = chars[i..end + close.chars().count()].iter().collect();
                    push(&mut spans, text, TokenClass::Comment);
                    i = end + close.chars().count();
                } else {
                    let text: String = chars[i..].iter().collect();
                    push(&mut spans, text, TokenClass::Comment);
                    *in_block_comment = true;
                    i = chars.len();
                }
                continue;
            }
        }

        // Line comment.
        if lang
            .line_comment
            .iter()
            .any(|marker| starts_with_at(&chars, i, marker))
        {
            let text: String = chars[i..].iter().collect();
            push(&mut spans, text, TokenClass::Comment);
            break;
        }

        // String literal.
        if c == '"' || c == '\'' {
            let quote = c;
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            push(&mut spans, text, TokenClass::String);
            continue;
        }

        // Shell variable: `$name`, `${name}`, `$1`, `$@`.
        if lang.shell_like && c == '$' {
            let start = i;
            i += 1;
            if i < chars.len() && chars[i] == '{' {
                i += 1;
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
            } else {
                while i < chars.len() && is_ident_continue(chars[i]) {
                    i += 1;
                }
                if i == start + 1 && i < chars.len() {
                    // `$@`, `$?`, `$1`, ... - a single special/positional char.
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            push(&mut spans, text, TokenClass::Function);
            continue;
        }

        // Shell flag: `-x`, `--long-flag`, only at the start of a word.
        if lang.shell_like
            && c == '-'
            && (i == 0 || chars[i - 1].is_whitespace())
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_alphanumeric() || *n == '-')
        {
            let start = i;
            i += 1;
            while i < chars.len() && (is_ident_continue(chars[i]) || chars[i] == '-') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            push(&mut spans, text, TokenClass::Keyword);
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            push(&mut spans, text, TokenClass::Number);
            continue;
        }

        // Identifier / keyword / command name / function call.
        if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident_continue(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let class = if lang.shell_like && first_word_is_function && !seen_word {
                TokenClass::Function
            } else if lang.keywords.contains(&word.as_str()) {
                TokenClass::Keyword
            } else if chars.get(i) == Some(&'(') {
                TokenClass::Function
            } else {
                TokenClass::Plain
            };
            seen_word = true;
            push(&mut spans, word, class);
            continue;
        }

        // Whitespace/punctuation/operators: consume a run, tracking "have we
        // seen the command word yet" so e.g. leading indentation doesn't
        // count as the shell command name.
        let start = i;
        while i < chars.len()
            && !is_ident_start(chars[i])
            && !chars[i].is_ascii_digit()
            && chars[i] != '"'
            && chars[i] != '\''
            && !(lang.shell_like && chars[i] == '$')
            && !(lang.shell_like
                && chars[i] == '-'
                && chars
                    .get(i + 1)
                    .is_some_and(|n| n.is_alphanumeric() || *n == '-'))
        {
            if lang
                .block_comment
                .is_some_and(|(open, _)| starts_with_at(&chars, i, open))
            {
                break;
            }
            if lang
                .line_comment
                .iter()
                .any(|marker| starts_with_at(&chars, i, marker))
            {
                break;
            }
            if !chars[i].is_whitespace() {
                seen_word = true;
            }
            i += 1;
        }
        if i == start {
            // Nothing matched above and none of the special-case guards
            // consumed this char (e.g. a lone `-` not at a word start) -
            // take it as plain so the scanner always makes progress.
            i += 1;
        }
        let text: String = chars[start..i].iter().collect();
        push(&mut spans, text, TokenClass::Plain);
    }

    spans
}

fn starts_with_at(chars: &[char], i: usize, needle: &str) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    if i + needle.len() > chars.len() {
        return false;
    }
    chars[i..i + needle.len()] == needle[..]
}

fn find_str(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || from > chars.len() {
        return None;
    }
    (from..=chars.len().saturating_sub(needle.len()))
        .find(|&i| chars[i..i + needle.len()] == needle[..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(line: &str, language: &str) -> Vec<(String, TokenClass)> {
        highlight_code(line, language)
            .into_iter()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.text, s.class))
            .collect()
    }

    #[test]
    fn rust_keyword_and_number() {
        let got = classes("let x = 1;", "rust");
        assert_eq!(
            got,
            vec![
                ("let".into(), TokenClass::Keyword),
                (" x = ".into(), TokenClass::Plain),
                ("1".into(), TokenClass::Number),
                (";".into(), TokenClass::Plain),
            ]
        );
    }

    #[test]
    fn rust_line_comment_takes_rest_of_line() {
        let got = classes("let x = 1; // comment", "rust");
        assert_eq!(
            got.last().unwrap(),
            &("// comment".into(), TokenClass::Comment)
        );
    }

    #[test]
    fn string_literal_with_escaped_quote() {
        let got = classes(r#"let s = "a\"b";"#, "rust");
        assert!(got.contains(&(r#""a\"b""#.into(), TokenClass::String)));
    }

    #[test]
    fn block_comment_spans_multiple_lines() {
        let out = highlight_code("/* start\nstill comment\nend */\nlet x = 1;", "rust");
        assert_eq!(out.len(), 4);
        assert!(out[0].iter().all(|s| s.class == TokenClass::Comment));
        assert!(out[1].iter().all(|s| s.class == TokenClass::Comment));
        assert!(out[2].iter().all(|s| s.class == TokenClass::Comment));
        assert_eq!(out[3][0].class, TokenClass::Keyword);
    }

    #[test]
    fn shell_command_name_flag_and_variable() {
        let got = highlight_shell_command("mkdir -p \"$HOME/Desktop\"");
        let got: Vec<(String, TokenClass)> = got.into_iter().map(|s| (s.text, s.class)).collect();
        assert_eq!(
            got,
            vec![
                ("mkdir".into(), TokenClass::Function),
                (" ".into(), TokenClass::Plain),
                ("-p".into(), TokenClass::Keyword),
                (" ".into(), TokenClass::Plain),
                (r#""$HOME/Desktop""#.into(), TokenClass::String),
            ]
        );
    }

    #[test]
    fn python_generic_and_json_keywords() {
        assert_eq!(
            classes("x = True", "python"),
            vec![
                ("x = ".into(), TokenClass::Plain),
                ("True".into(), TokenClass::Keyword),
            ]
        );
        assert_eq!(
            classes("null", "json"),
            vec![("null".into(), TokenClass::Keyword)]
        );
    }

    #[test]
    fn unknown_language_still_flags_strings_and_numbers() {
        let got = classes("value = 42 # note", "");
        assert!(got.contains(&("42".into(), TokenClass::Number)));
        assert!(got.contains(&("# note".into(), TokenClass::Comment)));
    }

    #[test]
    fn argv_word_classification() {
        assert_eq!(classify_shell_word("bash", true), TokenClass::Function);
        assert_eq!(classify_shell_word("-p", false), TokenClass::Keyword);
        assert_eq!(
            classify_shell_word("--long-flag", false),
            TokenClass::Keyword
        );
        assert_eq!(classify_shell_word("42", false), TokenClass::Number);
        assert_eq!(
            classify_shell_word("ls *.png | wc -l", false),
            TokenClass::String
        );
        assert_eq!(classify_shell_word("Desktop", false), TokenClass::Plain);
        assert_eq!(classify_shell_word("-1", false), TokenClass::Number);
    }
}

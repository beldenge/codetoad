use crossterm::style::Stylize;
use std::io::{self, Write};

const STREAM_FLUSH_THRESHOLD: usize = 16;

#[derive(Default)]
pub struct MarkdownStreamRenderer {
    pending: String,
    flushed_prefix: usize,
    in_code_block: bool,
    code_lang: String,
}

pub fn stream_markdown_chunk(renderer: &mut MarkdownStreamRenderer, chunk: &str) -> io::Result<()> {
    renderer.pending.push_str(chunk);

    while let Some(newline_idx) = renderer.pending.find('\n') {
        let line = renderer.pending[..newline_idx].to_string();
        let already_printed = renderer.flushed_prefix.min(line.len());
        if already_printed > 0 {
            let remainder = &line[already_printed..];
            print!("{remainder}");
        } else {
            render_markdown_line(renderer, &line)?;
        }
        println!();
        renderer.pending = renderer.pending[(newline_idx + 1)..].to_string();
        renderer.flushed_prefix = 0;
    }

    let unflushed = renderer
        .pending
        .len()
        .saturating_sub(renderer.flushed_prefix);
    if unflushed >= STREAM_FLUSH_THRESHOLD {
        let delta = &renderer.pending[renderer.flushed_prefix..];
        print!("{delta}");
        renderer.flushed_prefix = renderer.pending.len();
    }

    io::stdout().flush()
}

pub fn flush_markdown_pending(renderer: &mut MarkdownStreamRenderer) -> io::Result<()> {
    if renderer.pending.is_empty() {
        return Ok(());
    }

    let already_printed = renderer.flushed_prefix.min(renderer.pending.len());
    if already_printed > 0 {
        let remainder = &renderer.pending[already_printed..];
        print!("{remainder}");
    } else {
        let line = renderer.pending.clone();
        render_markdown_line(renderer, &line)?;
    }

    renderer.pending.clear();
    renderer.flushed_prefix = 0;
    io::stdout().flush()
}

fn render_markdown_line(renderer: &mut MarkdownStreamRenderer, line: &str) -> io::Result<()> {
    let trimmed = line.trim_start();

    if trimmed.starts_with("```") {
        if renderer.in_code_block {
            renderer.in_code_block = false;
            renderer.code_lang.clear();
        } else {
            renderer.in_code_block = true;
            renderer.code_lang = trimmed.trim_start_matches("```").trim().to_lowercase();
        }
        print!("{}", line.dark_grey());
        return Ok(());
    }

    if renderer.in_code_block {
        render_code_line(line, &renderer.code_lang)?;
        return Ok(());
    }

    if is_heading_line(trimmed) {
        print!("{}", line.cyan().bold());
        return Ok(());
    }

    if trimmed.starts_with("> ") {
        print!("{}", line.dark_grey());
        return Ok(());
    }

    if let Some((indent, marker, rest)) = split_list_prefix(line) {
        print!("{indent}");
        print!("{}", marker.cyan());
        render_inline_markdown(rest)?;
        return Ok(());
    }

    render_inline_markdown(line)
}

fn render_inline_markdown(line: &str) -> io::Result<()> {
    let mut in_code = false;
    let mut buf = String::new();

    for ch in line.chars() {
        if ch == '`' {
            if !buf.is_empty() {
                if in_code {
                    print!("{}", buf.as_str().yellow());
                } else {
                    print!("{buf}");
                }
                buf.clear();
            }
            in_code = !in_code;
            continue;
        }
        buf.push(ch);
    }

    if !buf.is_empty() {
        if in_code {
            print!("{}", buf.as_str().yellow());
        } else {
            print!("{buf}");
        }
    }

    io::stdout().flush()
}

fn render_code_line(line: &str, lang: &str) -> io::Result<()> {
    let mut chars = line.chars().peekable();
    let mut in_string: Option<char> = None;
    let mut string_buf = String::new();
    let mut word_buf = String::new();
    let comment_prefix = code_comment_prefix(lang);

    while let Some(ch) = chars.next() {
        if let Some(quote) = in_string {
            string_buf.push(ch);
            let escaped = string_buf
                .chars()
                .rev()
                .nth(1)
                .map(|c| c == '\\')
                .unwrap_or(false);
            if ch == quote && !escaped {
                print!("{}", string_buf.as_str().yellow());
                string_buf.clear();
                in_string = None;
            }
            continue;
        }

        if is_comment_start(ch, chars.peek().copied(), comment_prefix) {
            flush_code_word(&word_buf, lang);
            word_buf.clear();
            let mut comment = ch.to_string();
            if let Some(next) = chars.peek().copied()
                && ((comment_prefix == "//" && next == '/')
                    || (comment_prefix == "--" && next == '-'))
            {
                comment.push(chars.next().unwrap_or_default());
            }
            for c in chars {
                comment.push(c);
            }
            print!("{}", comment.dark_green());
            io::stdout().flush()?;
            return Ok(());
        }

        if ch == '"' || ch == '\'' {
            flush_code_word(&word_buf, lang);
            word_buf.clear();
            in_string = Some(ch);
            string_buf.push(ch);
            continue;
        }

        if ch.is_alphanumeric() || ch == '_' {
            word_buf.push(ch);
            continue;
        }

        flush_code_word(&word_buf, lang);
        word_buf.clear();
        print!("{}", ch.to_string().dark_cyan());
    }

    flush_code_word(&word_buf, lang);
    if !string_buf.is_empty() {
        print!("{}", string_buf.dark_yellow());
    }
    io::stdout().flush()
}

fn flush_code_word(word: &str, lang: &str) {
    if word.is_empty() {
        return;
    }
    if word.chars().all(|ch| ch.is_ascii_digit()) {
        print!("{}", word.dark_yellow());
    } else if is_lang_keyword(lang, word) {
        print!("{}", word.cyan().bold());
    } else {
        print!("{}", word.dark_cyan());
    }
}

fn code_comment_prefix(lang: &str) -> &'static str {
    match lang {
        "python" | "py" | "bash" | "sh" | "zsh" | "yaml" | "yml" | "toml" => "#",
        "sql" => "--",
        _ => "//",
    }
}

fn is_comment_start(current: char, next: Option<char>, prefix: &str) -> bool {
    match prefix {
        "#" => current == '#',
        "--" => current == '-' && next == Some('-'),
        _ => current == '/' && next == Some('/'),
    }
}

fn is_heading_line(trimmed: &str) -> bool {
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    level > 0 && level <= 6 && trimmed.chars().nth(level) == Some(' ')
}

fn split_list_prefix(line: &str) -> Option<(&str, &str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = &line[indent_len..];

    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some((indent, "- ", rest));
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some((indent, "* ", rest));
    }

    let mut chars = trimmed.chars().peekable();
    let mut digit_count = 0usize;
    while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
        chars.next();
        digit_count += 1;
    }
    if digit_count == 0 {
        return None;
    }
    if chars.next() != Some('.') || chars.next() != Some(' ') {
        return None;
    }

    let marker_len = digit_count + 2;
    let marker = &trimmed[..marker_len];
    let rest = &trimmed[marker_len..];
    Some((indent, marker, rest))
}

fn is_lang_keyword(lang: &str, word: &str) -> bool {
    match lang {
        "rust" | "rs" => matches!(
            word,
            "fn" | "let"
                | "mut"
                | "pub"
                | "struct"
                | "enum"
                | "impl"
                | "trait"
                | "match"
                | "if"
                | "else"
                | "for"
                | "while"
                | "loop"
                | "return"
                | "use"
                | "mod"
                | "async"
                | "await"
                | "where"
                | "const"
                | "static"
        ),
        "typescript" | "ts" | "javascript" | "js" | "tsx" | "jsx" => matches!(
            word,
            "function"
                | "const"
                | "let"
                | "var"
                | "return"
                | "if"
                | "else"
                | "for"
                | "while"
                | "class"
                | "import"
                | "export"
                | "from"
                | "async"
                | "await"
                | "try"
                | "catch"
                | "throw"
                | "new"
                | "interface"
                | "type"
        ),
        "python" | "py" => matches!(
            word,
            "def"
                | "class"
                | "return"
                | "if"
                | "elif"
                | "else"
                | "for"
                | "while"
                | "import"
                | "from"
                | "try"
                | "except"
                | "finally"
                | "with"
                | "as"
                | "async"
                | "await"
                | "lambda"
        ),
        "bash" | "sh" | "zsh" => matches!(
            word,
            "if" | "then" | "else" | "fi" | "for" | "do" | "done" | "case" | "esac" | "function"
        ),
        "json" => matches!(word, "true" | "false" | "null"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        code_comment_prefix, is_comment_start, is_heading_line, is_lang_keyword, split_list_prefix,
    };

    #[test]
    fn heading_detection_requires_hash_space_prefix() {
        assert!(is_heading_line("# Title"));
        assert!(is_heading_line("### Subtitle"));
        assert!(!is_heading_line("####### Too deep"));
        assert!(!is_heading_line("#NoSpace"));
        assert!(!is_heading_line("plain text"));
    }

    #[test]
    fn split_list_prefix_handles_bullets_and_numbered_lists() {
        assert_eq!(split_list_prefix("  - item"), Some(("  ", "- ", "item")));
        assert_eq!(split_list_prefix("* another"), Some(("", "* ", "another")));
        assert_eq!(
            split_list_prefix("12. numbered"),
            Some(("", "12. ", "numbered"))
        );
        assert_eq!(split_list_prefix("1) not-supported"), None);
        assert_eq!(split_list_prefix("no list"), None);
    }

    #[test]
    fn comment_prefix_matches_language_family() {
        assert_eq!(code_comment_prefix("python"), "#");
        assert_eq!(code_comment_prefix("sql"), "--");
        assert_eq!(code_comment_prefix("rust"), "//");
    }

    #[test]
    fn comment_start_detection_uses_selected_prefix() {
        assert!(is_comment_start('#', None, "#"));
        assert!(is_comment_start('-', Some('-'), "--"));
        assert!(is_comment_start('/', Some('/'), "//"));
        assert!(!is_comment_start('-', Some('x'), "--"));
        assert!(!is_comment_start('/', Some('*'), "//"));
    }

    #[test]
    fn language_keyword_detection_is_language_specific() {
        assert!(is_lang_keyword("rust", "fn"));
        assert!(!is_lang_keyword("rust", "function"));
        assert!(is_lang_keyword("typescript", "function"));
        assert!(is_lang_keyword("python", "lambda"));
        assert!(!is_lang_keyword("unknown", "fn"));
    }
}

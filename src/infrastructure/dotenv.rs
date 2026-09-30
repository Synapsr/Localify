//! Dotenv parser for launch profile sources.
//!
//! Follows the rules of the `dotenv` package that Next.js and Vite use, so a
//! file means the same thing whether the framework or Localify reads it:
//! optional `export`, single, double and backtick quotes, quoted values that
//! span lines, `#` comments, and the last duplicate key wins. Only `\n` and
//! `\r` are unescaped, and only inside double quotes. `$VAR` stays literal.
//!
//! Warnings carry line numbers only, never file content: these files hold
//! secrets.

use std::fmt;

/// Parsed dotenv content in first-appearance key order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DotenvFile {
    pub entries: Vec<(String, String)>,
    pub warnings: Vec<DotenvWarning>,
}

impl DotenvFile {
    fn set(&mut self, key: &str, value: String) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == key)
        {
            Some(entry) => entry.1 = value,
            None => self.entries.push((key.to_string(), value)),
        }
    }
}

/// A line Localify read differently from what its author probably meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DotenvWarning {
    /// Not a `KEY=value` assignment; skipped.
    InvalidLine { line: usize },
    /// Opening quote without a matching closing quote; read as plain text.
    UnterminatedQuote { line: usize },
    /// Unquoted value cut at `#`, which starts a comment in dotenv.
    HashInUnquotedValue { line: usize },
}

impl fmt::Display for DotenvWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine { line } => {
                write!(
                    f,
                    "line {line} is not a KEY=value assignment and was skipped"
                )
            }
            Self::UnterminatedQuote { line } => write!(
                f,
                "line {line} opens a quote that is never closed; its value was read as plain text"
            ),
            Self::HashInUnquotedValue { line } => write!(
                f,
                "line {line} has '#' in an unquoted value; dotenv treats the rest as a comment. Quote the value to keep it"
            ),
        }
    }
}

/// Parses dotenv content. Never fails: unreadable lines become warnings,
/// matching how frameworks skip them.
pub fn parse(content: &str) -> DotenvFile {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut file = DotenvFile::default();
    let mut index = 0;

    while index < lines.len() {
        let line_number = index + 1;
        let line = lines[index].trim_start();
        index += 1;

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, raw_value)) = split_assignment(line) else {
            file.warnings
                .push(DotenvWarning::InvalidLine { line: line_number });
            continue;
        };

        let parsed = parse_value(raw_value, &lines[index..], line_number);
        index += parsed.extra_lines;
        file.warnings.extend(parsed.warning);
        file.set(key, parsed.value);
    }

    file
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let line = strip_export(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim_end();
    is_valid_key(key).then_some((key, value))
}

fn strip_export(line: &str) -> &str {
    match line.strip_prefix("export") {
        Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
        _ => line,
    }
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

struct ParsedValue {
    value: String,
    extra_lines: usize,
    warning: Option<DotenvWarning>,
}

fn parse_value(raw: &str, following: &[&str], line: usize) -> ParsedValue {
    let raw = raw.trim_start();
    let quote = raw.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'));
    let Some(quote) = quote else {
        return parse_unquoted(raw, line);
    };

    match find_quoted_body(&raw[1..], quote, following) {
        Some(quoted) if is_blank_or_comment(&quoted.rest) => ParsedValue {
            value: if quote == '"' {
                quoted.body.replace("\\n", "\n").replace("\\r", "\r")
            } else {
                quoted.body
            },
            extra_lines: quoted.extra_lines,
            warning: None,
        },
        // dotenv falls back to the plain-text form of the first line when
        // text follows the closing quote.
        Some(_) => parse_unquoted(raw, line),
        None => ParsedValue {
            warning: Some(DotenvWarning::UnterminatedQuote { line }),
            ..parse_unquoted(raw, line)
        },
    }
}

struct QuotedBody {
    body: String,
    rest: String,
    extra_lines: usize,
}

/// Finds the first closing quote not preceded by a backslash, possibly on a
/// later line.
fn find_quoted_body(first: &str, quote: char, following: &[&str]) -> Option<QuotedBody> {
    let mut body = String::new();
    let lines = std::iter::once(first).chain(following.iter().copied());

    for (extra_lines, text) in lines.enumerate() {
        if extra_lines > 0 {
            body.push('\n');
        }
        let mut previous = None;
        for (offset, c) in text.char_indices() {
            if c == quote && previous != Some('\\') {
                body.push_str(&text[..offset]);
                return Some(QuotedBody {
                    body,
                    rest: text[offset + c.len_utf8()..].to_string(),
                    extra_lines,
                });
            }
            previous = Some(c);
        }
        body.push_str(text);
    }

    None
}

fn is_blank_or_comment(text: &str) -> bool {
    let text = text.trim_start();
    text.is_empty() || text.starts_with('#')
}

fn parse_unquoted(raw: &str, line: usize) -> ParsedValue {
    let (value, warning) = match raw.find('#') {
        Some(position) => {
            let glued = position == 0 || !raw[..position].ends_with([' ', '\t']);
            let warning = glued.then_some(DotenvWarning::HashInUnquotedValue { line });
            (&raw[..position], warning)
        }
        None => (raw, None),
    };

    ParsedValue {
        value: value.trim().to_string(),
        extra_lines: 0,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(file: &'a DotenvFile, key: &str) -> Option<&'a str> {
        file.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn reads_plain_assignments_and_skips_comments() {
        let file = parse("# comment\n\nA=1\n  B = two words  \nexport C=3\n");

        assert_eq!(value(&file, "A"), Some("1"));
        assert_eq!(value(&file, "B"), Some("two words"));
        assert_eq!(value(&file, "C"), Some("3"));
        assert_eq!(file.entries.len(), 3);
        assert!(file.warnings.is_empty());
    }

    #[test]
    fn keeps_special_characters_in_values() {
        let file = parse("A=p!a&s*s^w=rd\nB=\"va#lue\"\nC='x#y=z'\nD=$HOME/path\n");

        assert_eq!(value(&file, "A"), Some("p!a&s*s^w=rd"));
        assert_eq!(value(&file, "B"), Some("va#lue"));
        assert_eq!(value(&file, "C"), Some("x#y=z"));
        assert_eq!(value(&file, "D"), Some("$HOME/path"));
        assert!(file.warnings.is_empty());
    }

    #[test]
    fn strips_inline_comments_like_dotenv() {
        let file = parse("A=value # trailing comment\nB=\"quoted\" # comment\nC=\n");

        assert_eq!(value(&file, "A"), Some("value"));
        assert_eq!(value(&file, "B"), Some("quoted"));
        assert_eq!(value(&file, "C"), Some(""));
        assert!(file.warnings.is_empty());
    }

    #[test]
    fn warns_when_hash_cuts_an_unquoted_value() {
        let file = parse("COLOR=#fff\nSECRET=ab#cd\n");

        assert_eq!(value(&file, "COLOR"), Some(""));
        assert_eq!(value(&file, "SECRET"), Some("ab"));
        assert_eq!(
            file.warnings,
            vec![
                DotenvWarning::HashInUnquotedValue { line: 1 },
                DotenvWarning::HashInUnquotedValue { line: 2 },
            ]
        );
    }

    #[test]
    fn double_quotes_expand_only_newline_escapes() {
        let file = parse(r#"A="line1\nline2\tx\"y""#);

        assert_eq!(value(&file, "A"), Some("line1\nline2\\tx\\\"y"));
    }

    #[test]
    fn single_and_backtick_quotes_are_literal() {
        let file = parse("A='raw\\n $X'\nB=`it's \"fine\"`\n");

        assert_eq!(value(&file, "A"), Some("raw\\n $X"));
        assert_eq!(value(&file, "B"), Some("it's \"fine\""));
    }

    #[test]
    fn quoted_values_can_span_lines() {
        let file = parse("KEY=\"-----BEGIN KEY-----\nabc\n-----END KEY-----\"\nNEXT=1\n");

        assert_eq!(
            value(&file, "KEY"),
            Some("-----BEGIN KEY-----\nabc\n-----END KEY-----")
        );
        assert_eq!(value(&file, "NEXT"), Some("1"));
    }

    #[test]
    fn unterminated_quote_reads_the_line_as_plain_text() {
        let file = parse("A=\"abc\nB=2\n");

        assert_eq!(value(&file, "A"), Some("\"abc"));
        assert_eq!(value(&file, "B"), Some("2"));
        assert_eq!(
            file.warnings,
            vec![DotenvWarning::UnterminatedQuote { line: 1 }]
        );
    }

    #[test]
    fn last_duplicate_wins_in_first_appearance_order() {
        let file = parse("A=first\nB=b\nA=second\n");

        assert_eq!(
            file.entries,
            vec![("A".into(), "second".into()), ("B".into(), "b".into())]
        );
    }

    #[test]
    fn reports_invalid_lines_by_number_only() {
        let file = parse("A=1\nnot an assignment secret-value\nBAD KEY=x\n");

        assert_eq!(value(&file, "A"), Some("1"));
        assert_eq!(
            file.warnings,
            vec![
                DotenvWarning::InvalidLine { line: 2 },
                DotenvWarning::InvalidLine { line: 3 },
            ]
        );
        let rendered = file.warnings[0].to_string();
        assert!(!rendered.contains("secret-value"));
    }

    #[test]
    fn handles_windows_line_endings_and_export_edge_cases() {
        let file = parse("A=1\r\nexport=2\r\nexportB=3\r\n");

        assert_eq!(value(&file, "A"), Some("1"));
        assert_eq!(value(&file, "export"), Some("2"));
        assert_eq!(value(&file, "exportB"), Some("3"));
    }
}

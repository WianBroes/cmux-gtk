//! Line joining for pasted terminal text, mirroring cmux's `TerminalPasteLineJoin`.

/// The non-empty lines of `text` after trimming padding and shell continuations.
///
/// Lines are split on `\n`, `\r\n` and `\r`; surrounding spaces and tabs are dropped, and an
/// odd trailing run of backslashes loses its last backslash before the line is trimmed again.
pub fn lines(text: &str) -> Vec<String> {
    let mut retained = Vec::new();
    for line in split_lines(text) {
        let mut line = line.trim_matches(is_padding);
        if trailing_backslashes(line) % 2 == 1 {
            line = &line[..line.len() - 1];
            line = line.trim_end_matches(is_padding);
        }
        if !line.is_empty() {
            retained.push(line.to_string());
        }
    }
    retained
}

/// Whether the text keeps more than one non-empty line after [`lines`] normalization.
pub fn spans_multiple_lines(text: &str) -> bool {
    lines(text).len() > 1
}

/// The retained lines joined by single spaces, as one terminal paste line.
pub fn joined(text: &str) -> String {
    lines(text).join(" ")
}

/// Spaces and tabs are the only padding stripped from pasted lines.
fn is_padding(character: char) -> bool {
    character == ' ' || character == '\t'
}

/// Count the backslashes ending `line`, which decide whether it continues on the next line.
fn trailing_backslashes(line: &str) -> usize {
    line.chars().rev().take_while(|character| *character == '\\').count()
}

/// Split on `\n`, `\r\n` and lone `\r`, keeping empty fields for the caller to filter.
fn split_lines(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                result.push(&text[start..index]);
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                start = index;
            }
            b'\n' => {
                result.push(&text[start..index]);
                index += 1;
                start = index;
            }
            _ => index += 1,
        }
    }
    result.push(&text[start..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_plain_lines() {
        assert_eq!(joined("a\nb"), "a b");
    }

    #[test]
    fn joins_backslash_continuations() {
        assert_eq!(joined("  ls \\\n  -la\n"), "ls -la");
    }

    #[test]
    fn keeps_even_backslashes() {
        assert_eq!(joined("a\\\\\nb"), "a\\\\ b");
    }

    #[test]
    fn single_retained_line_is_not_multiple() {
        assert!(!spans_multiple_lines("one\n\n"));
    }

    #[test]
    fn splits_crlf() {
        assert_eq!(joined("x\r\ny"), "x y");
    }

    #[test]
    fn blank_input_has_no_lines() {
        assert_eq!(joined("\n  \n"), "");
    }
}

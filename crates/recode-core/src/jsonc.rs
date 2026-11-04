//! JSONC: JSON with `//` and `/* */` comments and trailing commas, the format
//! opencode and VS Code users expect for configuration files.

/// Strip comments and trailing commas, preserving string contents.
pub fn normalize(input: &str) -> String {
    // A UTF-8 BOM is common when files are written by Windows editors and
    // PowerShell; JSON.parse rejects it.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index] as char;
        if in_string {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == '\\' {
                escaped = true;
            } else if byte == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            '"' => {
                in_string = true;
                output.push(byte);
                index += 1;
            }
            '/' if index + 1 < bytes.len() && bytes[index + 1] == b'/' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            '/' if index + 1 < bytes.len() && bytes[index + 1] == b'*' => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    if bytes[index] == b'\n' {
                        output.push('\n');
                    }
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            ',' => {
                // Look ahead past whitespace/comments for a closing bracket.
                let mut lookahead = index + 1;
                loop {
                    while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
                        lookahead += 1;
                    }
                    if lookahead + 1 < bytes.len()
                        && bytes[lookahead] == b'/'
                        && bytes[lookahead + 1] == b'/'
                    {
                        while lookahead < bytes.len() && bytes[lookahead] != b'\n' {
                            lookahead += 1;
                        }
                        continue;
                    }
                    if lookahead + 1 < bytes.len()
                        && bytes[lookahead] == b'/'
                        && bytes[lookahead + 1] == b'*'
                    {
                        lookahead += 2;
                        while lookahead + 1 < bytes.len()
                            && !(bytes[lookahead] == b'*' && bytes[lookahead + 1] == b'/')
                        {
                            lookahead += 1;
                        }
                        lookahead = (lookahead + 2).min(bytes.len());
                        continue;
                    }
                    break;
                }
                if lookahead < bytes.len() && matches!(bytes[lookahead], b'}' | b']') {
                    // Drop the trailing comma.
                } else {
                    output.push(',');
                }
                index += 1;
            }
            _ => {
                output.push(byte);
                index += 1;
            }
        }
    }
    output
}

pub fn parse(input: &str) -> Result<serde_json::Value, String> {
    let normalized = normalize(input);
    serde_json::from_str(&normalized).map_err(|error| {
        let line = error.line();
        let column = error.column();
        format!("invalid JSON at line {line}, column {column}: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_line_and_block_comments() {
        let input = r#"{
            // model used by default
            "model": "anthropic/claude", /* inline */
            "list": [1, 2] // trailing
        }"#;
        let value = parse(input).unwrap();
        assert_eq!(value["model"], "anthropic/claude");
        assert_eq!(value["list"][1], 2);
    }

    #[test]
    fn strips_trailing_commas() {
        let value = parse(r#"{"a": [1, 2,], "b": {"c": 3,},}"#).unwrap();
        assert_eq!(value["a"].as_array().unwrap().len(), 2);
        assert_eq!(value["b"]["c"], 3);
    }

    #[test]
    fn preserves_comment_like_strings() {
        let value =
            parse(r#"{"url": "https://example.com/a//b", "text": "/* not a comment */"}"#).unwrap();
        assert_eq!(value["url"], "https://example.com/a//b");
        assert_eq!(value["text"], "/* not a comment */");
    }

    #[test]
    fn strips_utf8_bom() {
        let value = parse("\u{feff}{\"model\": \"openai/gpt-5\"}").unwrap();
        assert_eq!(value["model"], "openai/gpt-5");
    }

    #[test]
    fn preserves_escaped_quotes() {
        let value = parse(r#"{"text": "a \" // b"}"#).unwrap();
        assert_eq!(value["text"], "a \" // b");
    }
}

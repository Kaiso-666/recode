use unicode_width::UnicodeWidthChar;

/// A small multi-line editor. Cursor column is measured in `char`s, which is
/// stable across editing operations (display width is a rendering concern).
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
}

impl Input {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| line.trim().is_empty())
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn take(&mut self) -> String {
        let text = self.text();
        self.clear();
        text
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_owned).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn insert_char(&mut self, character: char) {
        let byte = self.byte_index();
        self.lines[self.row].insert(byte, character);
        self.col += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for (index, segment) in text.split('\n').enumerate() {
            if index > 0 {
                self.newline();
            }
            for character in segment.chars() {
                self.insert_char(character);
            }
        }
    }

    pub fn newline(&mut self) {
        let byte = self.byte_index();
        let tail = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let byte = self.byte_index();
            let previous = self.lines[self.row][..byte]
                .chars()
                .last()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.lines[self.row].replace_range(byte - previous..byte, "");
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&current);
        }
    }

    pub fn delete(&mut self) {
        let length = self.lines[self.row].chars().count();
        if self.col < length {
            let byte = self.byte_index();
            let next = self.lines[self.row][byte..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.lines[self.row].replace_range(byte..byte + next, "");
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn right(&mut self) {
        let length = self.lines[self.row].chars().count();
        if self.col < length {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    /// Display column (tabs expanded to 4, wide characters counted).
    pub fn display_col(&self) -> usize {
        self.lines[self.row]
            .chars()
            .take(self.col)
            .map(|character| {
                if character == '\t' {
                    4
                } else {
                    UnicodeWidthChar::width(character).unwrap_or(0).max(1)
                }
            })
            .sum()
    }

    fn byte_index(&self) -> usize {
        self.lines[self.row]
            .char_indices()
            .nth(self.col)
            .map(|(index, _)| index)
            .unwrap_or(self.lines[self.row].len())
    }

    /// Wrap the editor at `width` display columns, returning (line, start col)
    /// for each visual row plus the cursor's visual position.
    pub fn visual(&self, width: usize) -> (Vec<String>, usize, usize) {
        let mut visual = Vec::new();
        let mut cursor_row = 0usize;
        let mut cursor_col = 0usize;
        for (row, line) in self.lines.iter().enumerate() {
            let expanded = line.replace('\t', "    ");
            let mut start = 0usize;
            let chars: Vec<char> = expanded.chars().collect();
            let mut current = String::new();
            let mut current_width = 0usize;
            for (index, character) in chars.iter().enumerate() {
                let width_char = UnicodeWidthChar::width(*character).unwrap_or(0).max(1);
                if current_width + width_char > width.max(1) && current_width > 0 {
                    visual.push(std::mem::take(&mut current));
                    if row == self.row && index == self.col {
                        cursor_row = visual.len();
                        cursor_col = 0;
                    }
                    current_width = 0;
                    start = index;
                }
                current.push(*character);
                current_width += width_char;
                if row == self.row && index + 1 == self.col {
                    cursor_row = visual.len();
                    cursor_col = current_width;
                }
            }
            if row == self.row && self.col >= chars.len() {
                cursor_row = visual.len();
                cursor_col = current_width;
            }
            visual.push(current);
            let _ = start;
        }
        if visual.is_empty() {
            visual.push(String::new());
        }
        (visual, cursor_row, cursor_col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_newlines_and_merges_lines() {
        let mut input = Input::new();
        input.insert_str("hello\nworld");
        assert_eq!(input.text(), "hello\nworld");
        assert_eq!(input.row, 1);
        assert_eq!(input.col, 5);
        input.backspace();
        assert_eq!(input.text(), "hello\nworl");
        input.home();
        input.backspace();
        assert_eq!(input.text(), "helloworl");
    }

    #[test]
    fn handles_unicode_cursor() {
        let mut input = Input::new();
        input.insert_str("héllo");
        input.left();
        input.backspace();
        assert_eq!(input.text(), "hélo");
    }

    #[test]
    fn visual_wraps_long_lines() {
        let mut input = Input::new();
        input.insert_str("abcdefghij");
        let (lines, row, col) = input.visual(4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
        assert_eq!(row, 2);
        assert_eq!(col, 2);
    }
}

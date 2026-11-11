//! The input editor: a small multi-line text editor, independent from the
//! shell. The command is only sent to the shell when it is submitted.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

const MAX_UNDO: usize = 200;

#[derive(Default)]
pub struct Editor {
    text: String,
    /// Cursor position, in bytes within `text`.
    cursor: usize,
    /// The other end of the selection, if there is one.
    anchor: Option<usize>,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Workflow parameters still to fill in, which Tab cycles through.
    fields: Vec<Range<usize>>,
}

/// A displayed row: a piece of one line of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualRow {
    pub range: Range<usize>,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    fn checkpoint(&mut self) {
        if self.undo.len() == MAX_UNDO {
            self.undo.remove(0);
        }
        self.undo.push((self.text.clone(), self.cursor));
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        self.fields.clear();
        if let Some((text, cursor)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.text, text), self.cursor));
            self.cursor = cursor;
            self.anchor = None;
        }
    }

    pub fn redo(&mut self) {
        self.fields.clear();
        if let Some((text, cursor)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.text, text), self.cursor));
            self.cursor = cursor;
            self.anchor = None;
        }
    }

    /// Replaces the whole text (recalling a command from the history).
    pub fn set_text(&mut self, text: &str) {
        self.checkpoint();
        self.text = text.to_owned();
        self.cursor = self.text.len();
        self.anchor = None;
        self.fields.clear();
    }

    /// Replaces the whole text with a workflow and selects its first
    /// parameter. `fields` gives the location of each parameter.
    pub fn set_template(&mut self, text: &str, fields: Vec<Range<usize>>) {
        self.set_text(text);
        self.fields = fields;
        self.cursor = 0;
        self.next_field();
    }

    /// Forgets the workflow parameters: Tab becomes completion again.
    pub fn clear_fields(&mut self) {
        self.fields.clear();
    }

    pub fn has_fields(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Selects the next parameter (wrapping to the first after the last).
    /// Returns `false` if there is none.
    pub fn next_field(&mut self) -> bool {
        let position = self.selection().map_or(self.cursor, |selection| selection.end);
        let next = self
            .fields
            .iter()
            .find(|field| field.start >= position)
            .or(self.fields.first())
            .cloned();
        match next {
            Some(field) => {
                self.anchor = Some(field.start);
                self.cursor = field.end;
                true
            }
            None => false,
        }
    }

    /// Empties the editor and returns its content.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.anchor = None;
        self.undo.clear();
        self.redo.clear();
        self.fields.clear();
        std::mem::take(&mut self.text)
    }

    /// Replaces a range of the text and puts the cursor right after it.
    pub fn replace(&mut self, range: Range<usize>, with: &str) {
        self.checkpoint();
        // Workflow parameters follow the text: shifted when they come after
        // the edit, resized when the edit is inside them, forgotten when it
        // overlaps them.
        let grown = with.len() as isize - range.len() as isize;
        let shift = |offset: usize| (offset as isize + grown) as usize;
        self.fields = std::mem::take(&mut self.fields)
            .into_iter()
            .filter_map(|field| {
                if field.start >= range.end && field.start > range.start {
                    Some(shift(field.start)..shift(field.end))
                } else if range.start >= field.start && range.end <= field.end {
                    Some(field.start..shift(field.end))
                } else if field.end <= range.start {
                    Some(field)
                } else {
                    None
                }
            })
            .collect();
        self.text.replace_range(range.clone(), with);
        self.cursor = range.start + with.len();
        self.anchor = None;
    }

    /// Inserts text in place of the selection, or at the cursor.
    pub fn insert(&mut self, text: &str) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.replace(range, text);
    }

    /// Deletes the selection, or else the range from the cursor to `to`.
    fn delete_to(&mut self, to: usize) {
        let range = self
            .selection()
            .unwrap_or(self.cursor.min(to)..self.cursor.max(to));
        if !range.is_empty() {
            self.replace(range, "");
        }
    }

    pub fn backspace(&mut self) {
        self.delete_to(self.previous_char(self.cursor));
    }

    pub fn delete_forward(&mut self) {
        self.delete_to(self.next_char(self.cursor));
    }

    pub fn delete_word_back(&mut self) {
        self.delete_to(self.previous_word(self.cursor));
    }

    pub fn delete_to_line_start(&mut self) {
        self.delete_to(self.line_start(self.cursor));
    }

    pub fn delete_to_line_end(&mut self) {
        self.delete_to(self.line_end(self.cursor));
    }

    /// Deletes the selection and returns its text (cut).
    pub fn cut(&mut self) -> Option<String> {
        let range = self.selection()?;
        let text = self.text[range.clone()].to_owned();
        self.replace(range, "");
        Some(text)
    }

    /// Moves the cursor; `select` extends the selection instead of leaving it.
    pub fn move_to(&mut self, offset: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset.min(self.text.len());
    }

    pub fn move_left(&mut self, select: bool) {
        // Without Shift, the arrow goes to the edge of the selection.
        let target = match self.selection() {
            Some(range) if !select => range.start,
            _ => self.previous_char(self.cursor),
        };
        self.move_to(target, select);
    }

    pub fn move_right(&mut self, select: bool) {
        let target = match self.selection() {
            Some(range) if !select => range.end,
            _ => self.next_char(self.cursor),
        };
        self.move_to(target, select);
    }

    pub fn move_word_left(&mut self, select: bool) {
        self.move_to(self.previous_word(self.cursor), select);
    }

    pub fn move_word_right(&mut self, select: bool) {
        self.move_to(self.next_word(self.cursor), select);
    }

    pub fn move_line_start(&mut self, select: bool) {
        self.move_to(self.line_start(self.cursor), select);
    }

    pub fn move_line_end(&mut self, select: bool) {
        self.move_to(self.line_end(self.cursor), select);
    }

    /// Moves up one row. Returns `false` if the cursor is already at the top.
    pub fn move_up(&mut self, select: bool, columns: usize) -> bool {
        let (row, column) = self.cursor_position(columns);
        if row == 0 {
            return false;
        }
        self.move_to(self.offset_at(row - 1, column, columns), select);
        true
    }

    /// Moves down one row. Returns `false` if the cursor is already at the bottom.
    pub fn move_down(&mut self, select: bool, columns: usize) -> bool {
        let (row, column) = self.cursor_position(columns);
        if row + 1 >= self.layout(columns).len() {
            return false;
        }
        self.move_to(self.offset_at(row + 1, column, columns), select);
        true
    }

    fn previous_char(&self, offset: usize) -> usize {
        self.text[..offset].char_indices().next_back().map_or(0, |(index, _)| index)
    }

    fn next_char(&self, offset: usize) -> usize {
        self.text[offset..].chars().next().map_or(offset, |ch| offset + ch.len_utf8())
    }

    fn previous_word(&self, offset: usize) -> usize {
        let mut chars = self.text[..offset].char_indices().rev().peekable();
        while chars.next_if(|(_, ch)| !is_word(*ch)).is_some() {}
        let mut start = chars.peek().map_or(0, |(index, ch)| index + ch.len_utf8());
        while let Some((index, _)) = chars.next_if(|(_, ch)| is_word(*ch)) {
            start = index;
        }
        start
    }

    fn next_word(&self, offset: usize) -> usize {
        let mut chars = self.text[offset..].char_indices().peekable();
        while chars.next_if(|(_, ch)| !is_word(*ch)).is_some() {}
        while chars.next_if(|(_, ch)| is_word(*ch)).is_some() {}
        chars.peek().map_or(self.text.len(), |(index, _)| offset + index)
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..].find('\n').map_or(self.text.len(), |index| offset + index)
    }

    /// Splits the text into rows of at most `columns` cells.
    pub fn layout(&self, columns: usize) -> Vec<VisualRow> {
        let columns = columns.max(2);
        let mut rows = Vec::new();
        let mut line_start = 0;
        for line in self.text.split('\n') {
            let mut row_start = line_start;
            let mut width = 0;
            for (index, ch) in line.char_indices() {
                let char_width = ch.width().unwrap_or(0);
                if width + char_width > columns {
                    rows.push(VisualRow { range: row_start..line_start + index });
                    row_start = line_start + index;
                    width = 0;
                }
                width += char_width;
            }
            rows.push(VisualRow { range: row_start..line_start + line.len() });
            line_start += line.len() + 1;
        }
        rows
    }

    /// Row and column (in cells) of a position in the text.
    pub fn position_of(&self, offset: usize, columns: usize) -> (usize, usize) {
        let rows = self.layout(columns);
        // A position at the junction of two rows belongs to the second one,
        // except at the very end of the text.
        let row = rows
            .iter()
            .rposition(|row| row.range.start <= offset)
            .unwrap_or(0);
        let column = self.text[rows[row].range.start..offset]
            .chars()
            .map(|ch| ch.width().unwrap_or(0))
            .sum();
        (row, column)
    }

    pub fn cursor_position(&self, columns: usize) -> (usize, usize) {
        self.position_of(self.cursor, columns)
    }

    /// The text position closest to a row and a column.
    pub fn offset_at(&self, row: usize, column: usize, columns: usize) -> usize {
        let rows = self.layout(columns);
        let Some(visual) = rows.get(row.min(rows.len() - 1)) else {
            return self.text.len();
        };
        let mut width = 0;
        for (index, ch) in self.text[visual.range.clone()].char_indices() {
            let char_width = ch.width().unwrap_or(0);
            if width + char_width.div_ceil(2) > column {
                return visual.range.start + index;
            }
            width += char_width;
        }
        visual.range.end
    }
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.insert(text);
        editor
    }

    #[test]
    fn insert_replaces_selection() {
        let mut editor = editor("git status");
        editor.move_word_left(true);
        editor.insert("log");
        assert_eq!(editor.text(), "git log");
        assert_eq!(editor.cursor(), 7);
    }

    #[test]
    fn word_motions_handle_accents_and_punctuation() {
        let mut editor = editor("größe  --straße=über");
        editor.move_word_left(false);
        assert_eq!(&editor.text()[editor.cursor()..], "über");
        editor.move_word_left(false);
        assert_eq!(&editor.text()[editor.cursor()..], "straße=über");
        editor.move_to(0, false);
        editor.move_word_right(false);
        assert_eq!(&editor.text()[..editor.cursor()], "größe");
        editor.delete_word_back();
        assert_eq!(editor.text(), "  --straße=über");
    }

    #[test]
    fn undo_and_redo() {
        let mut editor = editor("ls");
        editor.insert(" -la");
        editor.undo();
        assert_eq!(editor.text(), "ls");
        editor.redo();
        assert_eq!(editor.text(), "ls -la");
    }

    #[test]
    fn layout_wraps_long_lines_and_keeps_newlines() {
        let editor = editor("abcdef\ngh");
        let rows = editor.layout(4);
        assert_eq!(rows, vec![
            VisualRow { range: 0..4 },
            VisualRow { range: 4..6 },
            VisualRow { range: 7..9 },
        ]);
        assert_eq!(editor.position_of(5, 4), (1, 1));
        assert_eq!(editor.cursor_position(4), (2, 2));
        assert_eq!(editor.offset_at(1, 1, 4), 5);
        assert_eq!(editor.offset_at(0, 99, 4), 4);
    }

    #[test]
    fn vertical_moves_report_edges() {
        let mut editor = editor("on\nfour");
        assert!(!editor.move_down(false, 80));
        assert!(editor.move_up(false, 80));
        assert_eq!(editor.cursor(), 2);
        assert!(!editor.move_up(false, 80));
    }

    #[test]
    fn workflow_fields_follow_edits() {
        let mut editor = Editor::default();
        // "cp SRC DST" with two parameters.
        editor.set_template("cp SRC DST", vec![3..6, 7..10]);
        assert_eq!(editor.selected_text(), Some("SRC"));

        // Typing replaces the selected parameter; the next one is shifted.
        editor.insert("a.txt");
        assert!(editor.next_field());
        assert_eq!(editor.selected_text(), Some("DST"));
        editor.insert("b");
        assert_eq!(editor.text(), "cp a.txt b");

        // Tab goes back to the first one, which followed the typing.
        assert!(editor.next_field());
        assert_eq!(editor.selected_text(), Some("a.txt"));

        // An edit straddling a parameter makes it forgotten.
        editor.select_all();
        editor.insert("ls");
        assert!(!editor.has_fields());
    }

    #[test]
    fn line_kills() {
        let mut editor = editor("on\nfour five");
        editor.move_word_left(false);
        editor.delete_to_line_start();
        assert_eq!(editor.text(), "on\nfive");
        editor.delete_to_line_end();
        assert_eq!(editor.text(), "on\n");
    }
}

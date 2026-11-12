//! A block: a command and its output.
//!
//! While the command runs, its output lives in an emulator (`LiveBlock`).
//! Once finished, it is frozen into logical lines (`Block`), much lighter,
//! which are re-wrapped to the window width when displayed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions, GridCell};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};

use crate::terminal::Listener;

/// A position in a block's output: displayed row, then column.
pub type RowCol = (usize, usize);

pub struct LiveBlock {
    pub id: u64,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub term: Term<Listener>,
    pub started: Instant,
    /// False until the shell has actually launched the command: the line is
    /// accepted, but zsh may still reject it or wait for more input.
    pub executing: bool,
}

impl LiveBlock {
    /// Number of rows to display: the whole scrollback, then the screen up to
    /// the last used line (or the cursor's line).
    pub fn row_count(&self) -> usize {
        let grid = self.term.grid();
        let last_used = last_used_line(&self.term).max(grid.cursor.point.line.0);
        grid.history_size() + last_used as usize + 1
    }

    /// The grid line matching a displayed row.
    pub fn line(&self, row: usize) -> Line {
        Line(row as i32 - self.term.grid().history_size() as i32)
    }

    pub fn cells(&self, row: usize) -> impl Iterator<Item = &Cell> {
        let grid_row = &self.term.grid()[self.line(row)];
        (0..grid_row.len()).map(move |column| &grid_row[Column(column)])
    }

    fn wraps(&self, row: usize) -> bool {
        let grid_row = &self.term.grid()[self.line(row)];
        grid_row.len() > 0 && grid_row[Column(grid_row.len() - 1)].flags.contains(Flags::WRAPLINE)
    }

    pub fn text(&self, range: Option<(RowCol, RowCol)>) -> String {
        let rows = self.row_count();
        collect_text(rows, range, |row| (self.cells(row).cloned().collect(), self.wraps(row)))
    }
}

pub struct Block {
    pub id: u64,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub exit: Option<i32>,
    pub duration: Option<Duration>,
    /// Logical lines: a line that is too long is not cut here.
    lines: Vec<Vec<Cell>>,
    /// The width `row_ends` was computed for.
    wrapped_columns: usize,
    /// For each logical line, the cumulative number of displayed rows.
    row_ends: Vec<usize>,
}

impl Block {
    /// Freezes the output of a finished block.
    pub fn freeze(live: LiveBlock, exit: Option<i32>) -> Block {
        let mut lines = snapshot(&live.term);
        if !live.executing {
            // Without execution, the output starts with the prompt's line
            // feed, and with some shells (bash) the echo of the command.
            let blank = lines.iter().take_while(|line| line.is_empty()).count();
            lines.drain(..blank);
            let echo = live
                .command
                .lines()
                .zip(&lines)
                .take_while(|(command, line)| line_text(line).trim_end() == command.trim_end())
                .count();
            lines.drain(..echo);
        }
        Block {
            id: live.id,
            command: live.command,
            cwd: live.cwd,
            exit,
            duration: live.executing.then(|| live.started.elapsed()),
            lines,
            wrapped_columns: 0,
            row_ends: Vec::new(),
        }
    }

    /// A block without a command, for what the shell prints before the first prompt.
    pub fn from_term(id: u64, term: &Term<Listener>) -> Block {
        Block {
            id,
            command: String::new(),
            cwd: None,
            exit: None,
            duration: None,
            lines: snapshot(term),
            wrapped_columns: 0,
            row_ends: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Recomputes the line wrapping if the width changed.
    pub fn wrap(&mut self, columns: usize) {
        let columns = columns.max(1);
        if self.wrapped_columns == columns {
            return;
        }
        self.wrapped_columns = columns;
        self.row_ends.clear();
        let mut total = 0;
        for line in &self.lines {
            total += line.len().div_ceil(columns).max(1);
            self.row_ends.push(total);
        }
    }

    /// Number of displayed rows, for the width passed to [`Block::wrap`].
    pub fn row_count(&self) -> usize {
        self.row_ends.last().copied().unwrap_or(0)
    }

    /// The cells of a row, and whether it continues on the next one.
    pub fn row(&self, row: usize) -> (&[Cell], bool) {
        let line_index = self.row_ends.partition_point(|&end| end <= row);
        let Some(line) = self.lines.get(line_index) else {
            return (&[], false);
        };
        let first_row = if line_index == 0 { 0 } else { self.row_ends[line_index - 1] };
        let start = (row - first_row) * self.wrapped_columns;
        let end = (start + self.wrapped_columns).min(line.len());
        (&line[start.min(end)..end], end < line.len())
    }

    pub fn text(&self, range: Option<(RowCol, RowCol)>) -> String {
        collect_text(self.row_count(), range, |row| {
            let (cells, wraps) = self.row(row);
            (cells.to_vec(), wraps)
        })
    }
}

/// The last screen line that contains something (-1 if the screen is empty).
fn last_used_line(term: &Term<Listener>) -> i32 {
    let grid = term.grid();
    (0..grid.screen_lines() as i32)
        .rev()
        .find(|&line| {
            let row = &grid[Line(line)];
            (0..row.len()).any(|column| !row[Column(column)].is_empty())
        })
        .unwrap_or(-1)
}

/// Copies an emulator's content into logical lines, without trailing blanks.
fn snapshot(term: &Term<Listener>) -> Vec<Vec<Cell>> {
    let grid = term.grid();
    let first = -(grid.history_size() as i32);
    let last = last_used_line(term);

    let mut lines = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    for line in first..=last {
        let row = &grid[Line(line)];
        let wraps = row.len() > 0 && row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE);
        current.extend((0..row.len()).map(|column| {
            let mut cell = row[Column(column)].clone();
            cell.flags.remove(Flags::WRAPLINE);
            cell
        }));
        if !wraps {
            while current.last().is_some_and(|cell| cell.is_empty()) {
                current.pop();
            }
            current.shrink_to_fit();
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The text of a range of rows (the whole block if `range` is `None`).
///
/// `row` returns the cells of a row and tells whether it continues on the
/// next one, in which case no line break is inserted.
fn collect_text(
    rows: usize,
    range: Option<(RowCol, RowCol)>,
    row: impl Fn(usize) -> (Vec<Cell>, bool),
) -> String {
    if rows == 0 {
        return String::new();
    }
    let ((first_row, first_column), (last_row, last_column)) =
        range.unwrap_or(((0, 0), (rows - 1, usize::MAX)));
    let last_row = last_row.min(rows - 1);

    let mut text = String::new();
    for index in first_row..=last_row {
        let (cells, wraps) = row(index);
        let from = if index == first_row { first_column } else { 0 };
        let to = if index == last_row { last_column.saturating_add(1) } else { usize::MAX };

        let mut line = String::new();
        for cell in cells.iter().take(to).skip(from) {
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            line.push(cell.c);
            line.extend(cell.zerowidth().into_iter().flatten());
        }

        if wraps && index != last_row {
            text.push_str(&line);
        } else {
            text.push_str(line.trim_end());
            if index != last_row {
                text.push('\n');
            }
        }
    }
    text
}

fn line_text(line: &[Cell]) -> String {
    line.iter().map(|cell| cell.c).collect()
}

use std::collections::VecDeque;

const DEFAULT_SCROLLBACK_LIMIT: usize = 10_000;
const MAX_CSI_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellStyle {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    pub character: char,
    pub style: CellStyle,
}

impl Cell {
    const fn blank(style: CellStyle) -> Self {
        Self {
            character: ' ',
            style,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParserState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

/// A small, dependency-free terminal state machine.
///
/// This first implementation supports a single screen, bounded scrollback,
/// UTF-8 code points, common cursor/erase CSI commands, and common SGR colors.
/// It intentionally does not yet implement full Unicode display widths,
/// alternate screens, mouse reporting, or the complete VT/DEC protocol.
pub struct Terminal {
    columns: usize,
    rows: usize,
    cells: Vec<Cell>,
    cursor_x: usize,
    cursor_y: usize,
    saved_cursor: Option<(usize, usize)>,
    cursor_visible: bool,
    current_style: CellStyle,
    scrollback: VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
    parser: ParserState,
    csi: Vec<u8>,
    utf8: Vec<u8>,
    tab_stops: Vec<bool>,
    wrap_pending: bool,
}

impl Terminal {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let current_style = CellStyle::default();
        let mut terminal = Self {
            columns,
            rows,
            cells: vec![Cell::blank(current_style); columns * rows],
            cursor_x: 0,
            cursor_y: 0,
            saved_cursor: None,
            cursor_visible: true,
            current_style,
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK_LIMIT,
            parser: ParserState::Ground,
            csi: Vec::with_capacity(16),
            utf8: Vec::with_capacity(4),
            tab_stops: vec![false; columns],
            wrap_pending: false,
        };
        terminal.reset_tab_stops();
        terminal
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn row(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.rows {
            return None;
        }
        let start = row * self.columns;
        Some(&self.cells[start..start + self.columns])
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scrollback_row(&self, row: usize) -> Option<&[Cell]> {
        self.scrollback.get(row).map(Vec::as_slice)
    }

    /// Feed a partial or complete byte stream from a PTY.
    /// Incomplete UTF-8 and escape sequences are retained for the next call.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.parser {
                ParserState::Ground => self.process_ground_byte(byte),
                ParserState::Escape => self.process_escape_byte(byte),
                ParserState::Csi => self.process_csi_byte(byte),
                ParserState::Osc => {
                    if byte == 0x07 {
                        self.parser = ParserState::Ground;
                    } else if byte == 0x1b {
                        self.parser = ParserState::OscEscape;
                    }
                }
                ParserState::OscEscape => {
                    self.parser = if byte == b'\\' {
                        ParserState::Ground
                    } else if byte == 0x1b {
                        ParserState::OscEscape
                    } else {
                        ParserState::Osc
                    };
                }
            }
        }
    }

    /// Resize the visible screen while preserving the overlapping top-left area.
    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.max(1);
        let rows = rows.max(1);
        if columns == self.columns && rows == self.rows {
            return;
        }

        let mut resized = vec![Cell::blank(self.current_style); columns * rows];
        let copy_rows = rows.min(self.rows);
        let copy_columns = columns.min(self.columns);
        for row in 0..copy_rows {
            let old_start = row * self.columns;
            let new_start = row * columns;
            resized[new_start..new_start + copy_columns]
                .copy_from_slice(&self.cells[old_start..old_start + copy_columns]);
        }

        self.columns = columns;
        self.rows = rows;
        self.cells = resized;
        self.cursor_x = self.cursor_x.min(columns - 1);
        self.cursor_y = self.cursor_y.min(rows - 1);
        self.wrap_pending = false;
        self.tab_stops.resize(columns, false);
        self.reset_tab_stops();
    }

    /// Reset screen contents and parser state to terminal defaults.
    pub fn reset(&mut self) {
        self.current_style = CellStyle::default();
        self.cells.fill(Cell::blank(self.current_style));
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.saved_cursor = None;
        self.cursor_visible = true;
        self.scrollback.clear();
        self.parser = ParserState::Ground;
        self.csi.clear();
        self.utf8.clear();
        self.wrap_pending = false;
        self.reset_tab_stops();
    }

    /// Return visible rows as text without trimming spaces, useful for snapshots.
    pub fn visible_text(&self) -> Vec<String> {
        (0..self.rows)
            .map(|row| {
                self.row(row)
                    .unwrap()
                    .iter()
                    .map(|cell| cell.character)
                    .collect()
            })
            .collect()
    }

    fn process_ground_byte(&mut self, byte: u8) {
        if !self.utf8.is_empty() {
            if (0x80..=0xbf).contains(&byte) {
                self.utf8.push(byte);
                let expected = utf8_sequence_length(self.utf8[0]);
                if self.utf8.len() == expected {
                    let decoded = std::str::from_utf8(&self.utf8)
                        .ok()
                        .and_then(|text| text.chars().next());
                    self.utf8.clear();
                    self.put_character(decoded.unwrap_or('\u{fffd}'));
                }
                return;
            }
            self.utf8.clear();
            self.put_character('\u{fffd}');
            self.process_ground_byte(byte);
            return;
        }

        match byte {
            0x1b => self.parser = ParserState::Escape,
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => {
                self.cursor_x = 0;
                self.wrap_pending = false;
            }
            b'\t' => self.tab(),
            0x08 => {
                self.cursor_x = self.cursor_x.saturating_sub(1);
                self.wrap_pending = false;
            }
            0x00..=0x1f | 0x7f => {}
            0x20..=0x7e => self.put_character(byte as char),
            0x80..=0xbf => self.put_character('\u{fffd}'),
            0xc2..=0xf4 => self.utf8.push(byte),
            _ => self.put_character('\u{fffd}'),
        }
    }

    fn process_escape_byte(&mut self, byte: u8) {
        self.parser = ParserState::Ground;
        match byte {
            b'[' => {
                self.csi.clear();
                self.parser = ParserState::Csi;
            }
            b']' => self.parser = ParserState::Osc,
            b'7' => self.saved_cursor = Some((self.cursor_x, self.cursor_y)),
            b'8' => {
                if let Some((x, y)) = self.saved_cursor {
                    self.set_cursor(x, y);
                }
            }
            b'D' => self.line_feed(),
            b'E' => {
                self.cursor_x = 0;
                self.line_feed();
            }
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn process_csi_byte(&mut self, byte: u8) {
        if (0x40..=0x7e).contains(&byte) {
            self.parser = ParserState::Ground;
            self.execute_csi(byte);
        } else if self.csi.len() < MAX_CSI_BYTES && (0x20..=0x3f).contains(&byte) {
            self.csi.push(byte);
        } else {
            self.parser = ParserState::Ground;
            self.csi.clear();
        }
    }

    fn execute_csi(&mut self, final_byte: u8) {
        let body = String::from_utf8_lossy(&self.csi).into_owned();
        let private = body.starts_with('?');
        let params_text = body.trim_start_matches(|character| {
            character == '?' || character == '>' || character == '!'
        });
        let params: Vec<Option<u16>> = if params_text.is_empty() {
            vec![None]
        } else {
            params_text
                .split(';')
                .map(|part| part.split(':').next().unwrap_or("").parse::<u16>().ok())
                .collect()
        };
        let p = |index: usize, default: usize| -> usize {
            params
                .get(index)
                .and_then(|value| *value)
                .map(usize::from)
                .filter(|value| *value != 0)
                .unwrap_or(default)
        };

        match final_byte {
            b'A' => self.cursor_y = self.cursor_y.saturating_sub(p(0, 1)),
            b'B' => self.cursor_y = (self.cursor_y + p(0, 1)).min(self.rows - 1),
            b'C' => self.cursor_x = (self.cursor_x + p(0, 1)).min(self.columns - 1),
            b'D' => self.cursor_x = self.cursor_x.saturating_sub(p(0, 1)),
            b'E' => {
                self.cursor_y = (self.cursor_y + p(0, 1)).min(self.rows - 1);
                self.cursor_x = 0;
            }
            b'F' => {
                self.cursor_y = self.cursor_y.saturating_sub(p(0, 1));
                self.cursor_x = 0;
            }
            b'G' | b'`' => self.cursor_x = p(0, 1).saturating_sub(1).min(self.columns - 1),
            b'd' => self.cursor_y = p(0, 1).saturating_sub(1).min(self.rows - 1),
            b'H' | b'f' => self.set_cursor(p(1, 1).saturating_sub(1), p(0, 1).saturating_sub(1)),
            b'J' => self.erase_display(params.first().and_then(|value| *value).unwrap_or(0)),
            b'K' => self.erase_line(params.first().and_then(|value| *value).unwrap_or(0)),
            b'm' => self.set_graphics_rendition(&params),
            b's' => self.saved_cursor = Some((self.cursor_x, self.cursor_y)),
            b'u' => {
                if let Some((x, y)) = self.saved_cursor {
                    self.set_cursor(x, y);
                }
            }
            b'S' => self.scroll_up(p(0, 1)),
            b'T' => self.scroll_down(p(0, 1)),
            b'h' | b'l' if private && params.contains(&Some(25)) => {
                self.cursor_visible = final_byte == b'h';
            }
            _ => {}
        }
        self.wrap_pending = false;
    }

    fn set_graphics_rendition(&mut self, params: &[Option<u16>]) {
        let values: Vec<u16> = if params.is_empty() {
            vec![0]
        } else {
            params.iter().map(|value| value.unwrap_or(0)).collect()
        };
        let mut index = 0;
        while index < values.len() {
            match values[index] {
                0 => self.current_style = CellStyle::default(),
                1 => self.current_style.bold = true,
                3 => self.current_style.italic = true,
                4 => self.current_style.underline = true,
                7 => self.current_style.inverse = true,
                22 => self.current_style.bold = false,
                23 => self.current_style.italic = false,
                24 => self.current_style.underline = false,
                27 => self.current_style.inverse = false,
                30..=37 => {
                    self.current_style.foreground =
                        TerminalColor::Indexed((values[index] - 30) as u8)
                }
                39 => self.current_style.foreground = TerminalColor::Default,
                40..=47 => {
                    self.current_style.background =
                        TerminalColor::Indexed((values[index] - 40) as u8)
                }
                49 => self.current_style.background = TerminalColor::Default,
                90..=97 => {
                    self.current_style.foreground =
                        TerminalColor::Indexed((values[index] - 90 + 8) as u8)
                }
                100..=107 => {
                    self.current_style.background =
                        TerminalColor::Indexed((values[index] - 100 + 8) as u8)
                }
                38 | 48 if index + 2 < values.len() && values[index + 1] == 5 => {
                    let color = TerminalColor::Indexed(values[index + 2].min(255) as u8);
                    if values[index] == 38 {
                        self.current_style.foreground = color;
                    } else {
                        self.current_style.background = color;
                    }
                    index += 2;
                }
                38 | 48 if index + 4 < values.len() && values[index + 1] == 2 => {
                    let color = TerminalColor::Rgb(
                        values[index + 2].min(255) as u8,
                        values[index + 3].min(255) as u8,
                        values[index + 4].min(255) as u8,
                    );
                    if values[index] == 38 {
                        self.current_style.foreground = color;
                    } else {
                        self.current_style.background = color;
                    }
                    index += 4;
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn put_character(&mut self, character: char) {
        if self.wrap_pending {
            self.cursor_x = 0;
            self.line_feed();
            self.wrap_pending = false;
        }
        let index = self.cursor_y * self.columns + self.cursor_x;
        self.cells[index] = Cell {
            character,
            style: self.current_style,
        };
        if self.cursor_x + 1 == self.columns {
            self.wrap_pending = true;
        } else {
            self.cursor_x += 1;
        }
    }

    fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_y + 1 < self.rows {
            self.cursor_y += 1;
        } else {
            self.scroll_up(1);
        }
    }

    fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cursor_y > 0 {
            self.cursor_y -= 1;
        } else {
            self.scroll_down(1);
        }
    }

    fn tab(&mut self) {
        self.wrap_pending = false;
        if let Some(next) = (self.cursor_x + 1..self.columns).find(|&column| self.tab_stops[column])
        {
            self.cursor_x = next;
        } else {
            self.cursor_x = self.columns - 1;
        }
    }

    fn set_cursor(&mut self, x: usize, y: usize) {
        self.cursor_x = x.min(self.columns - 1);
        self.cursor_y = y.min(self.rows - 1);
        self.wrap_pending = false;
    }

    fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            if self.cursor_y == self.rows - 1 && self.scrollback_limit > 0 {
                let first = self.cells[..self.columns].to_vec();
                if self.scrollback.len() == self.scrollback_limit {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(first);
            }
            self.cells.copy_within(self.columns.., 0);
            let blank_start = (self.rows - 1) * self.columns;
            self.cells[blank_start..].fill(Cell::blank(self.current_style));
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            let retained_cells = self.cells.len() - self.columns;
            self.cells.copy_within(0..retained_cells, self.columns);
            self.cells[..self.columns].fill(Cell::blank(self.current_style));
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let cursor = self.cursor_y * self.columns + self.cursor_x;
        match mode {
            0 => {
                self.cells[cursor..(self.cursor_y + 1) * self.columns]
                    .fill(Cell::blank(self.current_style));
                self.cells[(self.cursor_y + 1) * self.columns..]
                    .fill(Cell::blank(self.current_style));
            }
            1 => {
                self.cells[..cursor + 1].fill(Cell::blank(self.current_style));
                self.cells[self.cursor_y * self.columns..cursor]
                    .fill(Cell::blank(self.current_style));
            }
            2 | 3 => {
                self.cells.fill(Cell::blank(self.current_style));
                if mode == 3 {
                    self.scrollback.clear();
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let start = self.cursor_y * self.columns;
        let end = start + self.columns;
        match mode {
            0 => self.cells[start + self.cursor_x..end].fill(Cell::blank(self.current_style)),
            1 => self.cells[start..start + self.cursor_x + 1].fill(Cell::blank(self.current_style)),
            2 => self.cells[start..end].fill(Cell::blank(self.current_style)),
            _ => {}
        }
    }

    fn reset_tab_stops(&mut self) {
        self.tab_stops.fill(false);
        self.reset_tab_stops_from(0);
    }

    fn reset_tab_stops_from(&mut self, start: usize) {
        for column in (start..self.columns).filter(|column| column % 8 == 0) {
            self.tab_stops[column] = true;
        }
    }
}

fn utf8_sequence_length(first: u8) -> usize {
    match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{CellStyle, Terminal, TerminalColor};

    fn line(terminal: &Terminal, row: usize) -> String {
        terminal
            .row(row)
            .unwrap()
            .iter()
            .map(|cell| cell.character)
            .collect()
    }

    #[test]
    fn writes_text_and_wraps_at_screen_width() {
        let mut terminal = Terminal::new(4, 2);
        terminal.feed(b"abcde");
        assert_eq!(line(&terminal, 0), "abcd");
        assert_eq!(line(&terminal, 1), "e   ");
        assert_eq!(terminal.cursor(), (1, 1));
    }

    #[test]
    fn handles_carriage_return_and_line_feed() {
        let mut terminal = Terminal::new(6, 2);
        terminal.feed(b"abc\rZ\r\nQ");
        assert_eq!(line(&terminal, 0), "Zbc   ");
        assert_eq!(line(&terminal, 1), "Q     ");
    }

    #[test]
    fn parses_csi_cursor_position_and_erase_line() {
        let mut terminal = Terminal::new(8, 2);
        terminal.feed(b"123456\x1b[1;3H\x1b[Kx");
        assert_eq!(line(&terminal, 0), "12x     ");
        assert_eq!(terminal.cursor(), (3, 0));
    }

    #[test]
    fn applies_sgr_styles_and_resets_them() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b[1;38;2;10;20;30mA\x1b[0mB");
        let row = terminal.row(0).unwrap();
        assert_eq!(row[0].style.foreground, TerminalColor::Rgb(10, 20, 30));
        assert!(row[0].style.bold);
        assert_eq!(row[1].style, CellStyle::default());
    }

    #[test]
    fn accepts_utf8_split_across_feed_calls() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(&[0xe2, 0x98]);
        terminal.feed(&[0x83]);
        assert_eq!(terminal.row(0).unwrap()[0].character, '☃');
    }

    #[test]
    fn scrolls_oldest_line_into_bounded_scrollback() {
        let mut terminal = Terminal::new(3, 2);
        terminal.feed(b"one\r\ntwo\r\ntri\r\nfou");
        assert_eq!(terminal.scrollback_len(), 2);
        assert_eq!(terminal.scrollback_row(0).unwrap()[0].character, 'o');
        assert_eq!(line(&terminal, 0), "tri");
        assert_eq!(line(&terminal, 1), "fou");
    }

    #[test]
    fn resize_preserves_overlapping_screen_contents() {
        let mut terminal = Terminal::new(4, 2);
        terminal.feed(b"abcd\r\nef");
        terminal.resize(6, 3);
        assert_eq!(line(&terminal, 0), "abcd  ");
        assert_eq!(line(&terminal, 1), "ef    ");
        assert_eq!(line(&terminal, 2), "      ");
    }

    #[test]
    fn ignores_osc_sequences_and_toggles_cursor_visibility() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"a\x1b]0;title\x07b\x1b[?25l");
        assert_eq!(line(&terminal, 0), "ab      ");
        assert!(!terminal.cursor_visible());
    }
}

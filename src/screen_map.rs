use crate::cursor::{DataCursor, ScreenCursor};
use crate::textarea::TextArea;
use crate::util::num_digits;
use crate::wrap::{WrapMode, WrappedLine, effective_wrap_width, wrapped_rows};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScreenLine {
    pub wrapped: WrappedLine,
    pub screen_width: usize,
    pub cursor_max_col: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DataLine {
    pub first_screen_line: usize,
    pub screen_line_count: usize,
    pub pure_ascii: bool,
}

fn is_pure_ascii(line: &str) -> bool {
    line.is_ascii() && !line.contains('\t')
}

fn char_display_width(c: char, col: usize, tab_len: u8) -> usize {
    if c == '\t' {
        let tab = tab_len.max(1) as usize;
        let pad = tab - (col % tab);
        pad.max(1)
    } else {
        c.width().unwrap_or(0)
    }
}

fn display_width(text: &str, tab_len: u8) -> usize {
    let mut col = 0usize;
    for c in text.chars() {
        col += char_display_width(c, col, tab_len);
    }
    col
}

fn screen_col_for_char_offset(text: &str, char_offset: usize, tab_len: u8) -> usize {
    let mut col = 0usize;
    for c in text.chars().take(char_offset) {
        col += char_display_width(c, col, tab_len);
    }
    col
}

fn char_offset_for_screen_col(text: &str, screen_col: usize, tab_len: u8) -> usize {
    let mut col = 0usize;
    let mut chars = 0usize;
    for c in text.chars() {
        if col >= screen_col {
            break;
        }
        let width = char_display_width(c, col, tab_len);
        if col + width > screen_col {
            break;
        }
        col += width;
        chars += 1;
    }
    chars
}

impl TextArea<'_> {
    fn fallback_rows(&self) -> Vec<WrappedLine> {
        self.lines
            .iter()
            .enumerate()
            .map(|(row, line)| WrappedLine {
                row,
                start_byte: 0,
                end_byte: line.len(),
                start_col: 0,
                end_col: line.chars().count(),
                first_in_row: true,
                last_in_row: true,
            })
            .collect()
    }

    pub(crate) fn screen_map_load(&self) {
        let wrap_width = if self.wrap_mode() != WrapMode::None {
            let width = self.area.get().width;
            if width > 0 {
                let line_number_len = self
                    .line_number_style()
                    .map(|_| num_digits(self.lines.len()));
                Some(effective_wrap_width(width, line_number_len))
            } else {
                None
            }
        } else {
            None
        };

        let rows = match wrap_width {
            Some(width) => wrapped_rows(&self.lines, self.wrap_mode(), width, self.tab_length()),
            None => self.fallback_rows(),
        };

        let mut screen_lines = Vec::with_capacity(rows.len());
        let mut data_pointers = vec![DataLine::default(); self.lines.len()];
        let mut current_row = 0usize;

        for (screen_idx, wrapped) in rows.into_iter().enumerate() {
            while current_row < wrapped.row {
                current_row += 1;
            }

            if data_pointers[wrapped.row].screen_line_count == 0 {
                data_pointers[wrapped.row].first_screen_line = screen_idx;
                data_pointers[wrapped.row].pure_ascii = is_pure_ascii(&self.lines[wrapped.row]);
            }
            data_pointers[wrapped.row].screen_line_count += 1;

            let fragment = &self.lines[wrapped.row][wrapped.start_byte..wrapped.end_byte];
            let char_count = wrapped.end_col.saturating_sub(wrapped.start_col);
            let cursor_max_col = if wrapped.last_in_row {
                display_width(fragment, self.tab_length())
            } else {
                screen_col_for_char_offset(
                    fragment,
                    char_count.saturating_sub(1),
                    self.tab_length(),
                )
            };
            screen_lines.push(ScreenLine {
                wrapped,
                screen_width: display_width(fragment, self.tab_length()),
                cursor_max_col,
            });
        }

        *self.screen_lines.borrow_mut() = screen_lines;
        *self.data_pointers.borrow_mut() = data_pointers;
    }

    pub(crate) fn screen_lines_count(&self) -> usize {
        self.screen_lines.borrow().len()
    }

    /// Total number of on-screen (wrapped) rows the current content occupies,
    /// including any [`TextArea::set_top_padding`] rows. With soft-wrap enabled
    /// this exceeds [`TextArea::lines`]`().len()`. Useful as the content length
    /// when rendering an external scrollbar.
    pub fn screen_line_count(&self) -> usize {
        self.effective_top_padding() as usize + self.screen_lines_count()
    }

    /// The wrapped-row count of the text alone, without the
    /// [`TextArea::set_top_padding`] rows — what a caller needs to decide
    /// whether the text fits the pane before choosing the padding.
    pub fn text_screen_line_count(&self) -> usize {
        self.screen_lines_count()
    }

    /// The vertical scroll offset applied during the last render, as a top
    /// screen-row into the wrapped content — counting any
    /// [`TextArea::set_top_padding`] rows, so it is 0 with the padding fully in
    /// view. Pair with [`TextArea::screen_line_count`] to drive a scrollbar.
    /// Zero until the widget has been rendered once.
    pub fn scroll_offset(&self) -> u16 {
        self.viewport.scroll_top().0
    }

    /// The horizontal scroll offset (leftmost visible screen column) applied
    /// during the last render. Only nonzero for no-wrap fields whose text
    /// overflows the viewport; always zero when soft-wrap is enabled. Subtract
    /// it from [`TextArea::screen_cursor`]`().col` to get the caret's
    /// viewport-relative column (e.g. to place a native terminal cursor). Zero
    /// until the widget has been rendered once.
    pub fn horizontal_scroll_offset(&self) -> u16 {
        self.viewport.scroll_top().1
    }

    /// Map a screen position to a data cursor `(line, column)`. `screen_row` is
    /// an absolute wrapped-row index into the content (add [`TextArea::scroll_offset`]
    /// to a viewport-relative click row) and counts any
    /// [`TextArea::set_top_padding`] rows, so a position inside the padding maps
    /// to the first line. `screen_col` is a column within that row. Both are
    /// clamped to valid ranges, so out-of-bounds input snaps to the nearest cell
    /// rather than panicking.
    pub fn cursor_at_screen(&self, screen_row: usize, screen_col: usize) -> DataCursor {
        let count = self.screen_lines_count();
        if count == 0 {
            return DataCursor(0, 0);
        }
        let row = screen_row
            .saturating_sub(self.effective_top_padding() as usize)
            .min(count - 1);
        self.screen_to_array(ScreenCursor {
            row,
            col: screen_col,
            char: None,
            dc: None,
        })
    }

    pub(crate) fn screen_line_width(&self, row: usize) -> usize {
        self.screen_lines.borrow()[row].screen_width
    }

    pub(crate) fn screen_line_max_cursor_col(&self, row: usize) -> usize {
        self.screen_lines.borrow()[row].cursor_max_col
    }

    pub(crate) fn screen_line(&self, row: usize) -> ScreenLine {
        self.screen_lines.borrow()[row]
    }

    pub(crate) fn screen_to_array(&self, screen: ScreenCursor) -> DataCursor {
        if let Some(dc) = screen.dc {
            return dc;
        }

        let line = self.screen_line(screen.row);
        let fragment =
            &self.lines[line.wrapped.row][line.wrapped.start_byte..line.wrapped.end_byte];
        let char_offset = char_offset_for_screen_col(fragment, screen.col, self.tab_length());
        DataCursor(line.wrapped.row, line.wrapped.start_col + char_offset)
    }

    pub(crate) fn array_to_screen(&self, array: DataCursor) -> ScreenCursor {
        let data_line = &self.data_pointers.borrow()[array.0];
        let screen_lines = self.screen_lines.borrow();

        let mut screen_idx = data_line.first_screen_line;
        for idx in
            data_line.first_screen_line..data_line.first_screen_line + data_line.screen_line_count
        {
            let line = screen_lines[idx];
            let contains = if line.wrapped.last_in_row {
                line.wrapped.start_col <= array.1 && array.1 <= line.wrapped.end_col
            } else {
                line.wrapped.start_col <= array.1 && array.1 < line.wrapped.end_col
            };
            if contains {
                screen_idx = idx;
                break;
            }
        }

        let line = screen_lines[screen_idx];
        let fragment = &self.lines[array.0][line.wrapped.start_byte..line.wrapped.end_byte];
        let char_offset = array.1.saturating_sub(line.wrapped.start_col);
        let col = screen_col_for_char_offset(fragment, char_offset, self.tab_length());

        ScreenCursor {
            row: screen_idx,
            col,
            char: self.lines[array.0].chars().nth(array.1),
            dc: Some(array),
        }
    }

    pub(crate) fn data_cursor(&self, screen: ScreenCursor) -> DataCursor {
        screen.to_array_cursor(self)
    }

    pub(crate) fn increment_screen_cursor(&self, screen: ScreenCursor) -> ScreenCursor {
        let dc = self.data_cursor(screen);
        self.array_to_screen(DataCursor(dc.0, dc.1 + 1))
    }

    pub(crate) fn decrement_screen_cursor(&self, screen: ScreenCursor) -> ScreenCursor {
        let dc = self.data_cursor(screen);
        self.array_to_screen(DataCursor(dc.0, dc.1 - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_core::layout::Rect;

    fn make_textarea(lines: &[&str], wrap_mode: WrapMode, width: u16) -> TextArea<'static> {
        let mut textarea = TextArea::from(lines.iter().copied());
        textarea.set_wrap_mode(wrap_mode);
        textarea.area.set(Rect {
            x: 0,
            y: 0,
            width,
            height: 40,
        });
        textarea.screen_map_load();
        textarea
    }

    fn assert_round_trips(textarea: &TextArea<'_>) {
        for (row, line) in textarea.lines.iter().enumerate() {
            for col in 0..=line.chars().count() {
                let dc = DataCursor(row, col);
                let sc = textarea.array_to_screen(dc);
                assert_eq!(textarea.screen_to_array(sc), dc);

                let detached = ScreenCursor { dc: None, ..sc };
                assert_eq!(textarea.screen_to_array(detached), dc);
                assert_eq!(
                    textarea.array_to_screen(textarea.screen_to_array(detached)),
                    sc
                );
            }
        }
    }

    #[test]
    fn screen_map_round_trips_ascii_word_wrap() {
        let textarea = make_textarea(
            &[
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
                "Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
            ],
            WrapMode::Word,
            24,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }

    #[test]
    fn screen_map_round_trips_cjk_glyph_wrap() {
        let textarea = make_textarea(
            &["混合中文字符和ASCIIabc来验证屏幕坐标与数据坐标的往返转换。"],
            WrapMode::Glyph,
            12,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }

    #[test]
    fn screen_map_round_trips_multilingual_word_or_glyph_wrap() {
        let textarea = make_textarea(
            &[
                "ASCII and русский текст can share one buffer.",
                "第二行 mixed-width 内容 with English words.",
            ],
            WrapMode::WordOrGlyph,
            18,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }
}

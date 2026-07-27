use crate::util::{num_digits, spaces};
use ratatui_core::style::Style;
use ratatui_core::text::{Line, Span};
use std::borrow::Cow;
use unicode_width::UnicodeWidthChar as _;

// A styled byte range with a layering priority. The effective style of any byte is
// the style of the highest-priority range covering it, or the line's base style
// where no range applies. Priority — not nesting — decides overlaps, so a selection
// that straddles a syntax range still wins across the whole selection (a LIFO stack
// would drop the selection where the syntax range ends).
struct StyledRange {
    start: usize,
    end: usize,
    style: Style,
    prio: u8,
}

// Higher wins. Syntax is base styling; the interactive layers overlay it.
const PRIO_SYNTAX: u8 = 1;
const PRIO_SELECT: u8 = 2;
#[cfg(feature = "search")]
const PRIO_SEARCH: u8 = 3;
const PRIO_CURSOR: u8 = 4;

struct DisplayTextBuilder {
    tab_len: u8,
    width: usize,
    mask: Option<char>,
}

impl DisplayTextBuilder {
    fn new(tab_len: u8, mask: Option<char>) -> Self {
        Self {
            tab_len,
            width: 0,
            mask,
        }
    }

    fn build<'s>(&mut self, s: &'s str) -> Cow<'s, str> {
        if let Some(ch) = self.mask {
            // Note: We don't need to track width on masking text since width of tab character is fixed
            let masked = std::iter::repeat_n(ch, s.chars().count()).collect();
            return Cow::Owned(masked);
        }

        let tab = spaces(self.tab_len);
        let mut buf = String::new();
        for (i, c) in s.char_indices() {
            if c == '\t' {
                if buf.is_empty() {
                    buf.reserve(s.len());
                    buf.push_str(&s[..i]);
                }
                if self.tab_len > 0 {
                    let len = self.tab_len as usize - (self.width % self.tab_len as usize);
                    buf.push_str(&tab[..len]);
                    self.width += len;
                }
            } else {
                if !buf.is_empty() {
                    buf.push(c);
                }
                self.width += c.width().unwrap_or(0);
            }
        }

        if !buf.is_empty() {
            Cow::Owned(buf)
        } else {
            Cow::Borrowed(s)
        }
    }
}

pub struct LineHighlighter<'a> {
    line: &'a str,
    spans: Vec<Span<'a>>,
    ranges: Vec<StyledRange>, // TODO: Consider smallvec
    style_begin: Style,
    cursor_at_end: bool,
    cursor_style: Style,
    tab_len: u8,
    mask: Option<char>,
    select_at_end: bool,
    select_style: Style,
}

impl<'a> LineHighlighter<'a> {
    pub fn new(
        line: &'a str,
        cursor_style: Style,
        tab_len: u8,
        mask: Option<char>,
        select_style: Style,
    ) -> Self {
        Self {
            line,
            spans: vec![],
            ranges: vec![],
            style_begin: Style::default(),
            cursor_at_end: false,
            cursor_style,
            tab_len,
            mask,
            select_at_end: false,
            select_style,
        }
    }

    pub fn line_number(&mut self, row: usize, lnum_len: u8, style: Style) {
        let pad = spaces(lnum_len - num_digits(row + 1) + 1);
        self.spans
            .push(Span::styled(format!("{}{} ", pad, row + 1), style));
    }

    pub fn line_number_placeholder(&mut self, lnum_len: u8, style: Style) {
        self.spans.push(Span::styled(spaces(lnum_len + 2), style));
    }

    pub fn cursor_line(&mut self, cursor_col: usize, style: Style) {
        if let Some((start, c)) = self.line.char_indices().nth(cursor_col) {
            self.ranges.push(StyledRange {
                start,
                end: start + c.len_utf8(),
                style: self.cursor_style,
                prio: PRIO_CURSOR,
            });
        } else {
            self.cursor_at_end = true;
        }
        self.style_begin = style;
    }

    pub fn set_line_style(&mut self, style: Style) {
        self.style_begin = style;
    }

    #[cfg(feature = "search")]
    pub fn search(&mut self, matches: impl Iterator<Item = (usize, usize)>, style: Style) {
        for (start, end) in matches {
            if start != end {
                self.ranges.push(StyledRange {
                    start,
                    end,
                    style,
                    prio: PRIO_SEARCH,
                });
            }
        }
    }

    /// Apply caller-supplied base styling to byte ranges of the line (e.g. markdown
    /// syntax highlighting). Ranges layer beneath cursor/selection/search. Offsets
    /// are relative to the start of this line fragment. Ranges may straddle the
    /// interactive layers freely; priority (not nesting) resolves overlaps.
    ///
    /// A range is dropped unless both edges are char boundaries within the line:
    /// `into_spans` slices the raw line at them, and a slice through the middle of
    /// a multi-byte char panics mid-render.
    pub fn syntax(&mut self, ranges: impl Iterator<Item = (usize, usize, Style)>) {
        for (start, end, style) in ranges {
            if start < end
                && self.line.is_char_boundary(start)
                && self.line.is_char_boundary(end)
                && end <= self.line.len()
            {
                self.ranges.push(StyledRange {
                    start,
                    end,
                    style,
                    prio: PRIO_SYNTAX,
                });
            }
        }
    }

    pub fn selection(
        &mut self,
        current_row: usize,
        start_row: usize,
        start_off: usize,
        end_row: usize,
        end_off: usize,
    ) {
        let (start, end) = if current_row == start_row {
            if start_row == end_row {
                (start_off, end_off)
            } else {
                self.select_at_end = true;
                (start_off, self.line.len())
            }
        } else if current_row == end_row {
            (0, end_off)
        } else if start_row < current_row && current_row < end_row {
            self.select_at_end = true;
            (0, self.line.len())
        } else {
            return;
        };
        if start != end {
            self.ranges.push(StyledRange {
                start,
                end,
                style: self.select_style,
                prio: PRIO_SELECT,
            });
        }
    }

    pub fn selection_segment(&mut self, start_off: usize, end_off: usize, select_at_end: bool) {
        if start_off < end_off {
            self.ranges.push(StyledRange {
                start: start_off,
                end: end_off,
                style: self.select_style,
                prio: PRIO_SELECT,
            });
        }
        if select_at_end {
            self.select_at_end = true;
        }
    }

    pub fn into_spans(self) -> Line<'a> {
        let Self {
            line,
            mut spans,
            ranges,
            tab_len,
            style_begin,
            cursor_style,
            cursor_at_end,
            mask,
            select_at_end,
            select_style,
        } = self;
        let mut builder = DisplayTextBuilder::new(tab_len, mask);

        if ranges.is_empty() {
            let built = builder.build(line);
            if !built.is_empty() {
                spans.push(Span::styled(built, style_begin));
            }
            if cursor_at_end {
                spans.push(Span::styled(" ", cursor_style));
            } else if select_at_end {
                spans.push(Span::styled(" ", select_style));
            }
            return Line::from(spans);
        }

        // Cut the line into segments at every range edge; within a segment the set of
        // covering ranges is constant, so one style wins for the whole segment.
        let mut points: Vec<usize> = Vec::with_capacity(ranges.len() * 2 + 2);
        points.push(0);
        points.push(line.len());
        for r in &ranges {
            points.push(r.start);
            points.push(r.end);
        }
        points.sort_unstable();
        points.dedup();

        // Walk segments, resolve the highest-priority covering style, and coalesce
        // adjacent segments that resolve to the same style into one span (so the
        // display text — and the builder's tab-width bookkeeping — stays contiguous).
        let mut run_start = 0usize;
        let mut run_style = style_begin;
        let mut have_run = false;

        for win in points.windows(2) {
            let (a, b) = (win[0], win[1]);
            if a >= b {
                continue;
            }
            // Compose the covering ranges onto the base by *patching* in ascending
            // priority (so higher layers win, but only for the fields they set). This
            // keeps a lower layer's color where a higher layer leaves it unset — e.g.
            // an empty cursor style (native bar cursor) doesn't blank the syntax color
            // under it, and a selection tints syntax-colored text rather than erasing.
            let mut style = style_begin;
            for lvl in 1..=PRIO_CURSOR {
                for r in &ranges {
                    if r.prio == lvl && r.start <= a && b <= r.end {
                        style = style.patch(r.style);
                    }
                }
            }

            if have_run && style == run_style {
                continue;
            }
            if have_run {
                spans.push(Span::styled(builder.build(&line[run_start..a]), run_style));
            }
            run_start = a;
            run_style = style;
            have_run = true;
        }
        if have_run {
            spans.push(Span::styled(
                builder.build(&line[run_start..line.len()]),
                run_style,
            ));
        }

        if cursor_at_end {
            spans.push(Span::styled(" ", cursor_style));
        } else if select_at_end {
            spans.push(Span::styled(" ", select_style));
        }

        Line::from(spans)
    }
}

// Tests for spans don't work with tui-rs
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_core::style::Color;
    use std::fmt::Debug;
    use unicode_width::UnicodeWidthStr as _;

    fn build(text: &'static str, tab: u8, mask: Option<char>) -> Cow<'static, str> {
        DisplayTextBuilder::new(tab, mask).build(text)
    }

    #[track_caller]
    fn build_with_offset(offset: usize, text: &'static str, tab: u8) -> Cow<'static, str> {
        let mut b = DisplayTextBuilder::new(tab, None);
        b.width = offset;
        let built = b.build(text);
        let want = offset + built.as_ref().width();
        assert_eq!(b.width, want, "in={:?}, out={:?}", text, built); // Check post condition
        built
    }

    #[test]
    #[rustfmt::skip]
    fn line_display_text() {
        assert_eq!(&build(      "",  0,      None),                  "");
        assert_eq!(&build(      "",  4,      None),                  "");
        assert_eq!(&build(      "",  8,      None),                  "");
        assert_eq!(&build(      "",  0, Some('x')),                  "");
        assert_eq!(&build(      "",  4, Some('x')),                  "");
        assert_eq!(&build(      "",  8, Some('x')),                  "");
        assert_eq!(&build(     "a",  0,      None),                 "a");
        assert_eq!(&build(     "a",  4,      None),                 "a");
        assert_eq!(&build(     "a",  8,      None),                 "a");
        assert_eq!(&build(     "a",  0, Some('x')),                 "x");
        assert_eq!(&build(     "a",  4, Some('x')),                 "x");
        assert_eq!(&build(     "a",  8, Some('x')),                 "x");
        assert_eq!(&build(   "a\t",  0,      None),                 "a");
        assert_eq!(&build(   "a\t",  4,      None),              "a   ");
        assert_eq!(&build(   "a\t",  8,      None),          "a       ");
        assert_eq!(&build(   "a\t",  0, Some('x')),                "xx");
        assert_eq!(&build(   "a\t",  4, Some('x')),                "xx");
        assert_eq!(&build(   "a\t",  8, Some('x')),                "xx");
        assert_eq!(&build(    "\t",  0,      None),                "\t");
        assert_eq!(&build(    "\t",  4,      None),              "    ");
        assert_eq!(&build(    "\t",  8,      None),          "        ");
        assert_eq!(&build(    "\t",  0, Some('x')),                 "x");
        assert_eq!(&build(    "\t",  4, Some('x')),                 "x");
        assert_eq!(&build(    "\t",  8, Some('x')),                 "x");
        assert_eq!(&build(  "a\tb",  0,      None),                "ab");
        assert_eq!(&build(  "a\tb",  4,      None),             "a   b");
        assert_eq!(&build(  "a\tb",  8,      None),         "a       b");
        assert_eq!(&build(  "a\tb",  0, Some('x')),               "xxx");
        assert_eq!(&build(  "a\tb",  4, Some('x')),               "xxx");
        assert_eq!(&build(  "a\tb",  8, Some('x')),               "xxx");
        assert_eq!(&build("a\t\tb",  0,      None),                "ab");
        assert_eq!(&build("a\t\tb",  4,      None),         "a       b");
        assert_eq!(&build("a\t\tb",  8,      None), "a               b");
        assert_eq!(&build("a\t\tb",  0, Some('x')),              "xxxx");
        assert_eq!(&build("a\t\tb",  4, Some('x')),              "xxxx");
        assert_eq!(&build("a\t\tb",  8, Some('x')),              "xxxx");
        assert_eq!(&build("a\tb\tc", 0,      None),               "abc");
        assert_eq!(&build("a\tb\tc", 4,      None),         "a   b   c");
        assert_eq!(&build("a\tb\tc", 8,      None), "a       b       c");
        assert_eq!(&build("a\tb\tc", 0, Some('x')),             "xxxxx");
        assert_eq!(&build("a\tb\tc", 4, Some('x')),             "xxxxx");
        assert_eq!(&build("a\tb\tc", 8, Some('x')),             "xxxxx");
        assert_eq!(&build("ab\t\t",  0,      None),                "ab");
        assert_eq!(&build("ab\t\t",  4,      None),          "ab      ");
        assert_eq!(&build("ab\t\t",  8,      None),  "ab              ");
        assert_eq!(&build("abcd\t",  4,      None),          "abcd    ");
        assert_eq!(&build(  "あ\t",  0,      None),                "あ");
        assert_eq!(&build(  "あ\t",  4,      None),              "あ  ");
        assert_eq!(&build(  "🐶\t",  4,      None),              "🐶  ");
        assert_eq!(&build(  "あ\t",  4, Some('x')),                "xx");

        // When the start position of the text is not start of the line (#43)
        assert_eq!(&build_with_offset(1,         "", 0),           "");
        assert_eq!(&build_with_offset(1,        "a", 0),          "a");
        assert_eq!(&build_with_offset(1,       "あ", 0),         "あ");
        assert_eq!(&build_with_offset(1,       "\t", 4),        "   ");
        assert_eq!(&build_with_offset(1,      "a\t", 4),        "a  ");
        assert_eq!(&build_with_offset(1,     "あ\t", 4),        "あ ");
        assert_eq!(&build_with_offset(2,       "\t", 4),         "  ");
        assert_eq!(&build_with_offset(2,      "a\t", 4),         "a ");
        assert_eq!(&build_with_offset(2,     "あ\t", 4),     "あ    ");
        assert_eq!(&build_with_offset(3,      "a\t", 4),      "a    ");
        assert_eq!(&build_with_offset(4,       "\t", 4),       "    ");
        assert_eq!(&build_with_offset(4,      "a\t", 4),       "a   ");
        assert_eq!(&build_with_offset(4,     "あ\t", 4),       "あ  ");
        assert_eq!(&build_with_offset(5,       "\t", 4),        "   ");
        assert_eq!(&build_with_offset(5,      "a\t", 4),        "a  ");
        assert_eq!(&build_with_offset(5,     "あ\t", 4),        "あ ");
        assert_eq!(&build_with_offset(2,     "\t\t", 4),     "      ");
        assert_eq!(&build_with_offset(2,   "a\ta\t", 4),     "a a   ");
        assert_eq!(&build_with_offset(1, "あ\tあ\t", 4),    "あ あ  ");
        assert_eq!(&build_with_offset(2, "あ\tあ\t", 4), "あ    あ  ");
    }

    fn assert_spans<T: Debug>(lh: LineHighlighter, want: &[(&str, Style)], context: T) {
        let line = lh.into_spans();
        let have = line
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>();
        assert_eq!(&have, want, "Test case: {context:?}");
    }

    const DEFAULT: Style = Style::new();
    const CUR: Style = Style::new().bg(Color::Red); // Cursor
    #[allow(unused)]
    const SEARCH: Style = Style::new().bg(Color::Green);
    const SEL: Style = Style::new().bg(Color::Blue);
    const LINE: Style = Style::new().bg(Color::Gray);
    const LNUM: Style = Style::new().bg(Color::Yellow);

    #[test]
    fn into_spans_normal_line() {
        let tests = [
            ("", &[][..]),
            ("abc", &[("abc", DEFAULT)][..]),
            ("a\tb\tc", &[("a   b   c", DEFAULT)][..]),
        ];
        for test in tests {
            let (line, want) = test;
            let lh = LineHighlighter::new(line, CUR, 4, None, SEL);
            assert_spans(lh, want, test);
        }
    }

    #[test]
    fn into_spans_cursor_line() {
        let tests = [
            ("", 0, &[(" ", CUR)][..]),
            ("a", 0, &[("a", CUR)][..]),
            ("a", 1, &[("a", LINE), (" ", CUR)][..]),
            ("あいう", 0, &[("あ", CUR), ("いう", LINE)][..]),
            ("あいう", 1, &[("あ", LINE), ("い", CUR), ("う", LINE)][..]),
            ("あいう", 2, &[("あい", LINE), ("う", CUR)][..]),
            ("a\tb", 1, &[("a", LINE), ("   ", CUR), ("b", LINE)][..]),
        ];

        for test in tests {
            let (line, col, want) = test;
            let mut lh = LineHighlighter::new(line, CUR, 4, None, SEL);
            lh.cursor_line(col, LINE);
            assert_spans(lh, want, test);
        }
    }

    #[test]
    fn into_spans_line_number() {
        let tests = [
            (0, 1, &[(" 1 ", LNUM)][..]),
            (123, 3, &[(" 124 ", LNUM)][..]),
            (123, 5, &[("   124 ", LNUM)][..]),
        ];
        for test in tests {
            let (row, len, want) = test;
            let mut lh = LineHighlighter::new("", CUR, 4, None, SEL);
            lh.line_number(row, len, LNUM);
            assert_spans(lh, want, test);
        }
    }

    #[cfg(feature = "search")]
    #[test]
    fn into_spans_search() {
        let tests = [
            ("abcde", &[(0, 5)][..], &[("abcde", SEARCH)][..]),
            (
                "abcde",
                &[(0, 1), (2, 3), (4, 5)][..],
                &[
                    ("a", SEARCH),
                    ("b", DEFAULT),
                    ("c", SEARCH),
                    ("d", DEFAULT),
                    ("e", SEARCH),
                ][..],
            ),
            (
                "abcde",
                &[(1, 2), (3, 4)][..],
                &[
                    ("a", DEFAULT),
                    ("b", SEARCH),
                    ("c", DEFAULT),
                    ("d", SEARCH),
                    ("e", DEFAULT),
                ][..],
            ),
            (
                // Adjacent same-style ranges coalesce into a single span.
                "abcde",
                &[(0, 2), (2, 4), (4, 5)][..],
                &[("abcde", SEARCH)][..],
            ),
            ("abcde", &[(1, 1)][..], &[("abcde", DEFAULT)][..]),
            (
                "あいうえお",
                &[(0, 3), (6, 9), (12, 15)][..],
                &[
                    ("あ", SEARCH),
                    ("い", DEFAULT),
                    ("う", SEARCH),
                    ("え", DEFAULT),
                    ("お", SEARCH),
                ][..],
            ),
            (
                // The adjacent (2,3),(3,4) matches coalesce; the tab still expands.
                "\ta\tb\t",
                &[(0, 1), (2, 3), (3, 4)][..],
                &[
                    ("    ", SEARCH),
                    ("a", DEFAULT),
                    ("   b", SEARCH),
                    ("   ", DEFAULT),
                ][..],
            ),
        ];

        for test in tests {
            let (line, matches, want) = test;
            let mut lh = LineHighlighter::new(line, CUR, 4, None, SEL);
            lh.search(matches.iter().copied(), SEARCH);
            assert_spans(lh, want, test);
        }
    }

    #[test]
    fn into_spans_selection() {
        let tests = [
            // (line, (row, start_row, start_off, end_row, end_off), want)
            ("abc", (0, 1, 0, 2, 0), &[("abc", DEFAULT)][..]),
            ("abc", (1, 1, 0, 1, 1), &[("a", SEL), ("bc", DEFAULT)][..]),
            ("abc", (1, 1, 2, 1, 3), &[("ab", DEFAULT), ("c", SEL)][..]),
            ("abc", (1, 1, 0, 1, 3), &[("abc", SEL)][..]),
            ("abc", (1, 1, 0, 2, 0), &[("abc", SEL), (" ", SEL)][..]),
            (
                "abc",
                (1, 1, 2, 2, 0),
                &[("ab", DEFAULT), ("c", SEL), (" ", SEL)][..],
            ),
            ("abc", (1, 1, 3, 2, 0), &[("abc", DEFAULT), (" ", SEL)][..]),
            ("abc", (2, 1, 0, 3, 0), &[("abc", SEL), (" ", SEL)][..]),
            ("abc", (2, 1, 0, 2, 0), &[("abc", DEFAULT)][..]),
            ("abc", (2, 1, 0, 2, 2), &[("ab", SEL), ("c", DEFAULT)][..]),
            ("abc", (2, 1, 0, 2, 3), &[("abc", SEL)][..]),
            (
                "ab\t",
                (1, 1, 2, 2, 0),
                &[("ab", DEFAULT), ("  ", SEL), (" ", SEL)][..],
            ),
            ("a\tb", (2, 1, 0, 3, 0), &[("a   b", SEL), (" ", SEL)][..]),
            (
                "a\tb",
                (2, 1, 0, 2, 2),
                &[("a   ", SEL), ("b", DEFAULT)][..],
            ),
        ];

        for test in tests {
            let (line, (row, start_row, start_off, end_row, end_off), want) = test;
            let mut lh = LineHighlighter::new(line, CUR, 4, None, SEL);
            lh.selection(row, start_row, start_off, end_row, end_off);
            assert_spans(lh, want, test);
        }
    }

    #[test]
    fn into_spans_mixed_highlights() {
        let tests = [
            (
                "cursor on selection",
                {
                    let mut lh = LineHighlighter::new("abcde", CUR, 4, None, SEL);
                    lh.cursor_line(2, LINE);
                    lh.selection(0, 0, 1, 0, 4);
                    lh
                },
                &[("a", LINE), ("b", SEL), ("c", CUR), ("d", SEL), ("e", LINE)][..],
            ),
            #[cfg(feature = "search")]
            (
                "cursor + selection + search",
                {
                    let mut lh = LineHighlighter::new("abcdefg", CUR, 4, None, SEL);
                    lh.cursor_line(3, LINE);
                    lh.selection(0, 0, 2, 0, 5);
                    lh.search([(1, 2), (5, 6)].into_iter(), SEARCH);
                    lh
                },
                &[
                    ("a", LINE),
                    ("b", SEARCH),
                    ("c", SEL),
                    ("d", CUR),
                    ("e", SEL),
                    ("f", SEARCH),
                    ("g", LINE),
                ][..],
            ),
            (
                "selection + cursor at end",
                {
                    let mut lh = LineHighlighter::new("ab", CUR, 4, None, SEL);
                    lh.cursor_line(2, LINE);
                    lh.selection(0, 0, 1, 2, 0);
                    lh
                },
                &[("a", LINE), ("b", SEL), (" ", CUR)][..],
            ),
            (
                "cursor at start of selection",
                {
                    let mut lh = LineHighlighter::new("abcd", CUR, 4, None, SEL);
                    lh.cursor_line(1, LINE);
                    lh.selection(0, 0, 1, 0, 3);
                    lh
                },
                &[("a", LINE), ("b", CUR), ("c", SEL), ("d", LINE)][..],
            ),
            (
                "cursor at end of selection",
                {
                    let mut lh = LineHighlighter::new("abcd", CUR, 4, None, SEL);
                    lh.cursor_line(2, LINE);
                    lh.selection(0, 0, 1, 0, 3);
                    lh
                },
                &[("a", LINE), ("b", SEL), ("c", CUR), ("d", LINE)][..],
            ),
            (
                "cursor covers selection",
                {
                    let mut lh = LineHighlighter::new("abc", CUR, 4, None, SEL);
                    lh.cursor_line(1, LINE);
                    lh.selection(0, 0, 1, 0, 2);
                    lh
                },
                &[("a", LINE), ("b", CUR), ("c", LINE)][..],
            ),
        ];

        for (what, lh, want) in tests {
            assert_spans(lh, want, what);
        }
    }

    const SYN: Style = Style::new().fg(Color::Cyan); // Syntax base style

    #[test]
    fn into_spans_syntax() {
        // A syntax range spanning the whole line, with a plain default gap after it.
        let mut lh = LineHighlighter::new("abcde", CUR, 4, None, SEL);
        lh.syntax([(0usize, 3usize, SYN)].into_iter());
        assert_spans(lh, &[("abc", SYN), ("de", DEFAULT)], "syntax only");
    }

    #[test]
    fn into_spans_selection_over_syntax_tints_not_erases() {
        // Selection straddles the end of a syntax range. Under priority compositing
        // the selection's background covers the whole selection (styled text is never
        // "skipped" as a LIFO stack would drop it), and the syntax foreground shows
        // through where the two overlap.
        let mut lh = LineHighlighter::new("abcde", CUR, 4, None, SEL);
        lh.syntax([(0usize, 2usize, SYN)].into_iter()); // "ab"
        lh.selection(0, 0, 1, 0, 4); // select "bcd"
        let syn_sel = SYN.patch(SEL);
        assert_spans(
            lh,
            &[("a", SYN), ("b", syn_sel), ("cd", SEL), ("e", DEFAULT)],
            "selection tints syntax rather than erasing it",
        );
    }

    #[test]
    fn cursor_keeps_syntax_color_when_cursor_style_empty() {
        // The editor draws a native bar cursor and sets an empty cursor style, so the
        // char under the cursor must keep its syntax color rather than being blanked.
        let empty = Style::new();
        let mut lh = LineHighlighter::new("abc", empty, 4, None, SEL);
        lh.syntax([(0usize, 3usize, SYN)].into_iter());
        lh.cursor_line(1, empty);
        assert_spans(lh, &[("abc", SYN)], "empty cursor keeps syntax color");
    }

    #[test]
    fn nonempty_cursor_overlays_syntax() {
        // A non-empty cursor style (e.g. the selection block) still overlays, but the
        // syntax foreground survives where the cursor style leaves it unset.
        let mut lh = LineHighlighter::new("abc", CUR, 4, None, SEL);
        lh.syntax([(0usize, 3usize, SYN)].into_iter());
        lh.cursor_line(1, Style::new());
        let syn_cur = SYN.patch(CUR);
        assert_spans(
            lh,
            &[("a", SYN), ("b", syn_cur), ("c", SYN)],
            "cursor overlays but keeps syntax fg",
        );
    }

    #[test]
    fn syntax_drops_a_range_that_would_split_a_char() {
        // `into_spans` slices the raw line at range edges, so an offset inside a
        // multi-byte char would panic mid-render rather than misdraw.
        let mut lh = LineHighlighter::new("\u{3042}\u{3044}", CUR, 4, None, SEL);
        lh.syntax([(1usize, 3usize, SYN), (0usize, 20usize, SYN)].into_iter());
        assert_spans(lh, &[("\u{3042}\u{3044}", DEFAULT)], "both ranges rejected");
        // The well-formed range on the same text still applies.
        let mut lh = LineHighlighter::new("\u{3042}\u{3044}", CUR, 4, None, SEL);
        lh.syntax([(0usize, 3usize, SYN)].into_iter());
        assert_spans(
            lh,
            &[("\u{3042}", SYN), ("\u{3044}", DEFAULT)],
            "boundary range kept",
        );
    }
}

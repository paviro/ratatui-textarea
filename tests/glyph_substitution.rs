use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::widgets::Widget as _;
use ratatui_textarea::{CursorMove, TextArea, WrapMode};

fn render_lines(textarea: &TextArea<'_>, width: u16, height: u16) -> Vec<String> {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut buf = Buffer::empty(area);
    textarea.render(area, &mut buf);

    (0..height)
        .map(|y| {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buf[(x, y)].symbol());
            }
            line
        })
        .collect()
}

#[test]
fn substitutions_render_without_touching_the_buffer_text() {
    let mut textarea = TextArea::from(["tags:\"apple\""]);
    textarea.set_glyph_substitutions(vec![vec![(5, '['), (11, ']')]]);

    assert_eq!(render_lines(&textarea, 20, 1)[0], "tags:[apple]        ");
    // The text itself is unchanged, which is what keeps the query re-parseable
    // and the clipboard honest.
    assert_eq!(textarea.lines(), ["tags:\"apple\""]);

    textarea.clear_glyph_substitutions();
    assert_eq!(render_lines(&textarea, 20, 1)[0], "tags:\"apple\"        ");
}

/// The whole premise: the screen map measures the buffer, so a width-preserving
/// substitution must leave every column calculation exactly where it was.
#[test]
fn a_substitution_does_not_move_the_caret() {
    let plain = TextArea::from(["tags:\"apple\" and more"]);
    let mut substituted = TextArea::from(["tags:\"apple\" and more"]);
    substituted.set_glyph_substitutions(vec![vec![(5, ' '), (11, ' ')]]);

    for col in 0..24 {
        assert_eq!(
            plain.cursor_at_screen(0, col),
            substituted.cursor_at_screen(0, col),
            "click at column {col}"
        );
    }
    assert_eq!(plain.screen_cursor(), substituted.screen_cursor());
    assert_eq!(plain.screen_line_count(), substituted.screen_line_count());
}

#[test]
fn substitutions_land_on_both_sides_of_a_wrap() {
    let mut textarea = TextArea::from(["\"aaaa\" \"bbbb\""]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea.set_glyph_substitutions(vec![vec![(0, '['), (5, ']'), (7, '['), (12, ']')]]);

    let lines = render_lines(&textarea, 7, 3);
    assert_eq!(lines[0], "[aaaa] ");
    assert_eq!(lines[1], "[bbbb] ");
}

#[test]
fn substitutions_survive_horizontal_scroll() {
    let mut textarea = TextArea::from(["0123456789\"abc\""]);
    textarea.set_glyph_substitutions(vec![vec![(10, '['), (14, ']')]]);
    textarea.move_cursor(CursorMove::End);

    // The viewport is narrower than the line, so it has scrolled; the pill's
    // delimiters still occupy the cells their quotes did.
    let line = render_lines(&textarea, 8, 1)[0].clone();
    assert!(line.contains("[abc]"), "{line:?}");
}

/// Offsets go stale the moment the text changes, and the next frame renders
/// before the caller can recompute them.
#[test]
fn stale_offsets_after_an_edit_do_not_panic() {
    let mut textarea = TextArea::from(["\"ärger\""]);
    textarea.set_glyph_substitutions(vec![vec![(0, '['), (8, ']'), (2, '<')]]);
    textarea.move_cursor(CursorMove::End);
    textarea.delete_char();
    textarea.delete_char();
    textarea.delete_char();

    // Offset 8 now runs past the end and offset 2 lands inside `ä`; both go
    // inert, and the one still valid at 0 keeps working.
    let lines = render_lines(&textarea, 12, 1);
    assert_eq!(lines[0], "[ärg        ");
}

#[test]
fn out_of_range_rows_are_ignored() {
    let mut textarea = TextArea::from(["\"a\"", "\"b\""]);
    // One entry for a line that does not exist, and none for the second line.
    textarea.set_glyph_substitutions(vec![vec![(0, '[')], vec![], vec![(0, '!')]]);

    let lines = render_lines(&textarea, 5, 2);
    assert_eq!(lines[0], "[a\"  ");
    assert_eq!(lines[1], "\"b\"  ");
}

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, TextStyle};

/// Files larger than this are opened as a read-only windowed view.
pub const EDIT_LIMIT: u64 = 2 * 1024 * 1024;

const LOOKBACK: u64 = 256 * 1024;
const MAX_WINDOW: usize = 256 * 1024;
const CHUNK: usize = 64 * 1024;

pub enum Opened {
    Edit(String),
    View(LargeView),
    Converted { text: String, txt_path: PathBuf },
}

#[derive(Debug)]
pub enum FileError {
    Open(std::io::Error),
    Read(std::io::Error),
    InvalidUtf8,
    InvalidRtf,
}

pub fn open(path: &Path) -> Result<Opened, FileError> {
    let size = fs::metadata(path).map_err(FileError::Open)?.len();
    if crate::rtf::path_is_rtf(path) {
        if size > EDIT_LIMIT {
            return Err(FileError::InvalidRtf);
        }
        let bytes = fs::read(path).map_err(FileError::Open)?;
        let text = crate::rtf::to_text(&bytes).map_err(|_| FileError::InvalidRtf)?;
        return Ok(Opened::Converted {
            text,
            txt_path: path.with_extension("txt"),
        });
    }
    if size > EDIT_LIMIT {
        Ok(Opened::View(LargeView::open(path, size)?))
    } else {
        let bytes = fs::read(path).map_err(FileError::Open)?;
        let text = String::from_utf8(bytes).map_err(|_| FileError::InvalidUtf8)?;
        Ok(Opened::Edit(text))
    }
}

pub struct LargeView {
    pub path: PathBuf,
    pub size: u64,
    offset: u64,
    window: String,
    prepared_for: Option<(u64, usize, usize)>,
}

impl LargeView {
    fn open(path: &Path, size: u64) -> Result<Self, FileError> {
        File::open(path).map_err(FileError::Open)?;
        Ok(Self {
            path: path.to_path_buf(),
            size,
            offset: 0,
            window: String::new(),
            prepared_for: None,
        })
    }

    pub fn status(&self, view_readonly: &str, decimal: char) -> String {
        let percent = if self.size == 0 {
            0
        } else {
            ((self.offset as f64 / self.size as f64) * 100.0).round() as u64
        };
        format!(
            "{} · {} % · {view_readonly}",
            format_size_with(self.size, decimal),
            percent.min(100)
        )
    }

    pub fn show(&mut self, ui: &mut egui::Ui, dialog_busy: bool, cannot_read: &str) {
        let row_height = ui.text_style_height(&TextStyle::Monospace);
        let height = ui.available_height();
        let rows = ((height / row_height).floor() as usize).max(1);
        let bar_w = 14.0;
        let text_w = (ui.available_width() - bar_w).max(40.0);
        let wrap_cols = wrap_columns(ui, text_w);

        if let Err(err) = self.ensure_window(rows, wrap_cols) {
            let message = match err {
                FileError::Open(err) | FileError::Read(err) => format!("{cannot_read}:\n{err}"),
                FileError::InvalidUtf8 | FileError::InvalidRtf => cannot_read.to_owned(),
            };
            ui.colored_label(Color32::from_rgb(160, 40, 40), message);
            return;
        }

        if !dialog_busy {
            self.handle_input(
                ui,
                rows,
                wrap_cols,
                row_height,
                ui.rect_contains_pointer(ui.max_rect()),
            );
        }

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(text_w, height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    ui.add(
                        egui::Label::new(RichText::new(&self.window).monospace())
                            .selectable(true)
                            .extend(),
                    );
                },
            );

            if let Some(new_offset) = file_scrollbar(ui, self.offset, self.size, height) {
                self.offset = new_offset;
                self.prepared_for = None;
                let _ = self.ensure_window(rows, wrap_cols);
            }
        });
    }

    fn handle_input(
        &mut self,
        ui: &mut egui::Ui,
        rows: usize,
        wrap_cols: usize,
        row_height: f32,
        hovered: bool,
    ) {
        if hovered {
            let scroll_y = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll_y.abs() >= 1.0 {
                let lines = (scroll_y / row_height).round() as i64;
                if lines != 0 {
                    let _ = self.scroll_lines(-lines, rows, wrap_cols);
                }
            }
        }

        if ui.input(|input| {
            input.key_pressed(egui::Key::PageDown)
                || (input.key_pressed(egui::Key::ArrowDown) && input.modifiers.command)
        }) {
            let _ = self.scroll_lines(rows as i64, rows, wrap_cols);
        } else if ui.input(|input| {
            input.key_pressed(egui::Key::PageUp)
                || (input.key_pressed(egui::Key::ArrowUp) && input.modifiers.command)
        }) {
            let _ = self.scroll_lines(-(rows as i64), rows, wrap_cols);
        } else if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            let _ = self.scroll_lines(1, rows, wrap_cols);
        } else if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            let _ = self.scroll_lines(-1, rows, wrap_cols);
        } else if ui.input(|input| input.key_pressed(egui::Key::Home)) {
            self.offset = 0;
            self.prepared_for = None;
            let _ = self.ensure_window(rows, wrap_cols);
        } else if ui.input(|input| input.key_pressed(egui::Key::End)) {
            let _ = self.scroll_to_end(rows, wrap_cols);
        }
    }

    fn ensure_window(&mut self, rows: usize, wrap_cols: usize) -> Result<(), FileError> {
        if self.prepared_for == Some((self.offset, rows, wrap_cols)) {
            return Ok(());
        }
        let mut file = File::open(&self.path).map_err(FileError::Read)?;
        self.size = file
            .metadata()
            .map(|meta| meta.len())
            .unwrap_or(self.size);
        self.offset = self.offset.min(self.size);
        if self.offset > 0 {
            self.offset = utf8_floor(&mut file, self.offset).map_err(FileError::Read)?;
        }
        self.window = read_window(&mut file, self.offset, self.size, rows, wrap_cols)
            .map_err(FileError::Read)?;
        self.prepared_for = Some((self.offset, rows, wrap_cols));
        Ok(())
    }

    fn scroll_lines(&mut self, delta: i64, rows: usize, wrap_cols: usize) -> Result<(), FileError> {
        if delta == 0 {
            return Ok(());
        }
        let mut file = File::open(&self.path).map_err(FileError::Read)?;
        self.offset = move_by_lines(&mut file, self.offset, self.size, delta, wrap_cols)
            .map_err(FileError::Read)?;
        self.prepared_for = None;
        self.ensure_window(rows, wrap_cols)
    }

    fn scroll_to_end(&mut self, rows: usize, wrap_cols: usize) -> Result<(), FileError> {
        let mut file = File::open(&self.path).map_err(FileError::Read)?;
        self.offset =
            move_by_lines(&mut file, self.size, self.size, -(rows as i64), wrap_cols)
                .map_err(FileError::Read)?;
        self.prepared_for = None;
        self.ensure_window(rows, wrap_cols)
    }
}

fn wrap_columns(ui: &egui::Ui, text_w: f32) -> usize {
    let font_id = TextStyle::Monospace.resolve(ui.style());
    let char_w = ui
        .fonts_mut(|fonts| fonts.glyph_width(&font_id, 'M'))
        .max(1.0);
    ((text_w / char_w).floor() as usize).max(8)
}

fn scrollbar_thumb_height(track: f32) -> f32 {
    if !track.is_finite() || track <= 0.0 {
        return 0.0;
    }
    let min_thumb = 20.0_f32.min(track);
    (track * 0.08).clamp(min_thumb, track)
}

fn file_scrollbar(ui: &mut egui::Ui, offset: u64, size: u64, height: f32) -> Option<u64> {
    let height = if height.is_finite() { height.max(0.0) } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, height), Sense::click_and_drag());
    let visuals = ui.visuals();
    ui.painter()
        .rect_filled(rect, 3.0, visuals.extreme_bg_color);

    let t = if size == 0 {
        0.0
    } else {
        (offset as f64 / size as f64).clamp(0.0, 1.0) as f32
    };
    let thumb_h = scrollbar_thumb_height(rect.height());
    let travel = (rect.height() - thumb_h).max(0.0);
    let thumb = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + t * travel),
        egui::vec2(rect.width(), thumb_h),
    );
    let thumb_fill = if response.hovered() || response.dragged() {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    ui.painter().rect_filled(thumb, 3.0, thumb_fill);

    if response.clicked() || response.dragged() {
        let pos = response.interact_pointer_pos()?;
        let f = ((pos.y - rect.top() - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0) as f64;
        Some((f * size as f64) as u64)
    } else {
        None
    }
}

fn utf8_floor(file: &mut File, offset: u64) -> std::io::Result<u64> {
    if offset == 0 {
        return Ok(0);
    }
    let back = offset.min(3);
    file.seek(SeekFrom::Start(offset - back))?;
    let mut buf = [0u8; 4];
    let n = file.read(&mut buf)?;
    let at = back as usize;
    if n <= at || buf[at] & 0xC0 != 0x80 {
        return Ok(offset);
    }
    let mut i = at;
    while i > 0 && buf[i] & 0xC0 == 0x80 {
        i -= 1;
    }
    Ok(offset - back + i as u64)
}

fn read_window(
    file: &mut File,
    start: u64,
    size: u64,
    rows: usize,
    wrap_cols: usize,
) -> std::io::Result<String> {
    if start >= size {
        return Ok(String::new());
    }
    let want = MAX_WINDOW.min((size - start) as usize);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; want];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    skip_incomplete_tail(&mut buf);

    let wrap_cols = wrap_cols.max(1);
    let mut out = String::new();
    let mut col = 0usize;
    let mut lines = 0usize;
    let mut i = 0usize;
    while i < buf.len() && lines < rows {
        match next_step(&buf, i) {
            None => break,
            Some(Step::Skip(w)) => i += w,
            Some(Step::Char(ch, w)) => {
                i += w;
                if ch == '\n' {
                    out.push('\n');
                    col = 0;
                    lines += 1;
                } else {
                    if col >= wrap_cols {
                        lines += 1;
                        if lines >= rows {
                            break;
                        }
                        out.push('\n');
                        col = 0;
                    }
                    out.push(ch);
                    col += 1;
                }
            }
        }
    }
    Ok(out)
}

fn skip_incomplete_tail(buf: &mut Vec<u8>) {
    let mut i = buf.len();
    let mut cont = 0;
    while i > 0 && buf[i - 1] & 0xC0 == 0x80 {
        i -= 1;
        cont += 1;
    }
    if i == 0 {
        return;
    }
    let lead = buf[i - 1];
    let need = utf8_width(lead);
    if need == 0 || cont + 1 != need {
        buf.truncate(i - 1);
    }
}

fn utf8_width(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else if lead & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}

enum Step {
    Char(char, usize),
    Skip(usize),
}

fn next_step(buf: &[u8], i: usize) -> Option<Step> {
    let (ch, w) = decode_char(buf, i)?;
    if ch == '\r' {
        if buf.get(i + w) == Some(&b'\n') {
            return Some(Step::Skip(w));
        }
        return Some(Step::Char('\n', w));
    }
    Some(Step::Char(ch, w))
}

fn decode_char(buf: &[u8], i: usize) -> Option<(char, usize)> {
    let first = *buf.get(i)?;
    if first < 0x80 {
        return Some((first as char, 1));
    }
    let width = utf8_width(first);
    if width < 2 || i + width > buf.len() {
        return Some(('\u{FFFD}', 1));
    }
    match std::str::from_utf8(&buf[i..i + width]) {
        Ok(s) => Some((s.chars().next()?, width)),
        Err(_) => Some(('\u{FFFD}', 1)),
    }
}

fn move_by_lines(
    file: &mut File,
    pos: u64,
    size: u64,
    delta: i64,
    wrap_cols: usize,
) -> std::io::Result<u64> {
    let wrap_cols = wrap_cols.max(1);
    if delta > 0 {
        scan_forward(file, pos, size, delta as u64, wrap_cols)
    } else if delta < 0 {
        scan_backward(file, pos, (-delta) as u64, wrap_cols)
    } else {
        Ok(pos.min(size))
    }
}

fn scan_forward(
    file: &mut File,
    pos: u64,
    size: u64,
    lines: u64,
    wrap_cols: usize,
) -> std::io::Result<u64> {
    if lines == 0 || pos >= size {
        return Ok(pos.min(size));
    }
    let cap = ((lines as usize).saturating_mul(wrap_cols).saturating_mul(4))
        .saturating_add(CHUNK)
        .min(MAX_WINDOW)
        .min((size - pos) as usize);
    file.seek(SeekFrom::Start(pos))?;
    let mut buf = vec![0u8; cap];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    skip_incomplete_tail(&mut buf);

    let mut left = lines;
    let mut col = 0usize;
    let mut i = 0usize;
    while i < buf.len() && left > 0 {
        match next_step(&buf, i) {
            None => break,
            Some(Step::Skip(w)) => i += w,
            Some(Step::Char(ch, w)) => {
                i += w;
                if ch == '\n' {
                    left -= 1;
                    col = 0;
                } else {
                    col += 1;
                    if col >= wrap_cols {
                        left -= 1;
                        col = 0;
                    }
                }
            }
        }
    }
    Ok((pos + i as u64).min(size))
}

fn scan_backward(
    file: &mut File,
    pos: u64,
    lines: u64,
    wrap_cols: usize,
) -> std::io::Result<u64> {
    if lines == 0 {
        return Ok(pos);
    }
    if pos == 0 {
        return Ok(0);
    }
    let cap = ((lines as usize).saturating_mul(wrap_cols).saturating_mul(4))
        .saturating_add(CHUNK)
        .min(LOOKBACK as usize);
    let start = pos.saturating_sub(cap as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; (pos - start) as usize];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    let mut i = 0usize;
    if start > 0 {
        while i < buf.len() && i < 3 && buf[i] & 0xC0 == 0x80 {
            i += 1;
        }
    }
    skip_incomplete_tail(&mut buf);

    let mut row_starts = vec![start + i as u64];
    let mut col = 0usize;
    while i < buf.len() {
        if start + i as u64 >= pos {
            break;
        }
        match next_step(&buf, i) {
            None => break,
            Some(Step::Skip(w)) => i += w,
            Some(Step::Char(ch, w)) => {
                i += w;
                let next_abs = start + i as u64;
                if ch == '\n' {
                    col = 0;
                    if next_abs <= pos {
                        row_starts.push(next_abs);
                    }
                } else {
                    col += 1;
                    if col >= wrap_cols {
                        col = 0;
                        if next_abs <= pos {
                            row_starts.push(next_abs);
                        }
                    }
                }
            }
        }
    }

    let idx = row_starts.len().saturating_sub(1);
    let target = idx.saturating_sub(lines as usize);
    Ok(row_starts[target])
}

pub fn format_size(bytes: u64) -> String {
    format_size_with(bytes, ',')
}

pub fn format_size_with(bytes: u64, decimal: char) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let (value, unit) = if bytes >= GB as u64 {
        (bytes as f64 / GB, "GB")
    } else if bytes >= MB as u64 {
        (bytes as f64 / MB, "MB")
    } else if bytes >= 1024 {
        (bytes as f64 / KB, "KB")
    } else {
        return format!("{bytes} B");
    };
    format!("{value:.1} {unit}").replace('.', &decimal.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rtf_opens_converted_with_txt_path() {
        let path = temp("note.rtf");
        write_all(&path, br"{\rtf1\ansi\pard Hello {\b world}\par}");
        match open(&path).unwrap() {
            Opened::Converted { text, txt_path } => {
                assert_eq!(text, "Hello world");
                assert_eq!(txt_path.extension().and_then(|e| e.to_str()), Some("txt"));
            }
            _ => panic!("expected converted rtf"),
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn small_files_open_for_edit() {
        let path = temp("small.txt");
        write_all(&path, b"abc\n");
        match open(&path).unwrap() {
            Opened::Edit(text) => assert_eq!(text, "abc\n"),
            Opened::View(_) | Opened::Converted { .. } => panic!("expected edit"),
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn files_over_limit_open_as_view() {
        let path = temp("over-limit.txt");
        let data = vec![b'x'; (EDIT_LIMIT as usize) + 8];
        write_all(&path, &data);
        match open(&path).unwrap() {
            Opened::View(view) => assert_eq!(view.size, EDIT_LIMIT + 8),
            Opened::Edit(_) | Opened::Converted { .. } => panic!("expected view"),
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn utf8_floor_does_not_rewind_complete_chars() {
        let path = temp("utf8-floor.txt");
        write_all(&path, b"abc\xc3\xa9xyz");
        let mut file = File::open(&path).unwrap();
        assert_eq!(utf8_floor(&mut file, 0).unwrap(), 0);
        assert_eq!(utf8_floor(&mut file, 3).unwrap(), 3);
        assert_eq!(utf8_floor(&mut file, 4).unwrap(), 3);
        assert_eq!(utf8_floor(&mut file, 5).unwrap(), 5);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn move_forward_and_back_over_lines() {
        let path = temp("move.txt");
        write_all(&path, b"one\ntwo\nthree\n");
        let mut file = File::open(&path).unwrap();
        let size = 14;
        let wrap = 1000;
        let two = scan_forward(&mut file, 0, size, 1, wrap).unwrap();
        assert_eq!(two, 4);
        let three = scan_forward(&mut file, two, size, 1, wrap).unwrap();
        assert_eq!(three, 8);
        assert_eq!(scan_backward(&mut file, three, 1, wrap).unwrap(), 4);
        assert_eq!(scan_backward(&mut file, two, 1, wrap).unwrap(), 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn window_stops_after_requested_rows() {
        let path = temp("window.txt");
        write_all(&path, b"a\nb\nc\nd\n");
        let mut file = File::open(&path).unwrap();
        let text = read_window(&mut file, 0, 8, 2, 1000).unwrap();
        assert_eq!(text, "a\nb\n");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn long_lines_wrap_to_fill_rows() {
        let path = temp("long.txt");
        let data = vec![b'z'; 50];
        write_all(&path, &data);
        let mut file = File::open(&path).unwrap();
        let text = read_window(&mut file, 0, data.len() as u64, 3, 10).unwrap();
        assert_eq!(text, "zzzzzzzzzz\nzzzzzzzzzz\nzzzzzzzzzz");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn first_physical_line_does_not_hide_later_lines() {
        let path = temp("wrap-then-lines.txt");
        let mut data = vec![b'a'; 200];
        data.extend_from_slice(b"\nsecond\nthird\n");
        write_all(&path, &data);
        let mut file = File::open(&path).unwrap();
        let text = read_window(&mut file, 0, data.len() as u64, 6, 80).unwrap();
        assert!(text.contains("second"), "{text:?}");
        assert!(text.contains("third"), "{text:?}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn scan_without_newlines_moves_by_wrap() {
        let path = temp("nonewline.txt");
        let data = vec![b'x'; MAX_WINDOW + CHUNK];
        write_all(&path, &data);
        let mut file = File::open(&path).unwrap();
        let size = data.len() as u64;
        let wrap = 80;
        let pos = scan_forward(&mut file, 0, size, 1, wrap).unwrap();
        assert_eq!(pos, wrap as u64);
        assert_eq!(scan_backward(&mut file, pos, 1, wrap).unwrap(), 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn giant_line_scroll_does_not_snap_home() {
        let path = temp("issue3.txt");
        let data: Vec<u8> = (0..LOOKBACK as usize + 4096)
            .map(|i| b'a' + (i % 26) as u8)
            .collect();
        write_all(&path, &data);
        let mut view = LargeView::open(&path, data.len() as u64).unwrap();
        let wrap = 80usize;
        let rows = 20usize;
        view.ensure_window(rows, wrap).unwrap();
        assert_eq!(view.offset, 0);
        view.scroll_lines(1, rows, wrap).unwrap();
        assert_eq!(view.offset, wrap as u64);
        let first = view.window.clone();
        view.scroll_lines(1, rows, wrap).unwrap();
        assert_eq!(view.offset, (wrap * 2) as u64);
        assert_ne!(view.window, first);
        assert_eq!(view.window.as_bytes()[0], data[wrap * 2]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn scrollbar_thumb_fits_short_track() {
        assert_eq!(scrollbar_thumb_height(18.0), 18.0);
        assert_eq!(scrollbar_thumb_height(200.0), 20.0);
        assert_eq!(scrollbar_thumb_height(400.0), 32.0);
        assert_eq!(scrollbar_thumb_height(0.0), 0.0);
    }

    #[test]
    fn format_size_uses_comma() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5,0 GB");
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ravnpad-{name}"))
    }

    fn write_all(path: &Path, bytes: &[u8]) {
        let mut file = File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }
}

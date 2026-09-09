use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, TextStyle};

/// Files larger than this are opened as a read-only windowed view.
pub const EDIT_LIMIT: u64 = 2 * 1024 * 1024;

const LOOKBACK: u64 = 256 * 1024;
const MAX_WINDOW: usize = 256 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const CHUNK: usize = 64 * 1024;

pub enum Opened {
    Edit(String),
    View(LargeView),
}

pub fn open(path: &Path) -> Result<Opened, String> {
    let size = fs::metadata(path)
        .map_err(|err| format!("Kunne ikke åpne filen:\n{err}"))?
        .len();
    if size > EDIT_LIMIT {
        Ok(Opened::View(LargeView::open(path, size)?))
    } else {
        let bytes = fs::read(path).map_err(|err| format!("Kunne ikke åpne filen:\n{err}"))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| "Filen er ikke gyldig UTF-8-tekst.".to_owned())?;
        Ok(Opened::Edit(text))
    }
}

pub struct LargeView {
    pub path: PathBuf,
    pub size: u64,
    offset: u64,
    window: String,
    prepared_for: Option<(u64, usize)>,
}

impl LargeView {
    fn open(path: &Path, size: u64) -> Result<Self, String> {
        File::open(path).map_err(|err| format!("Kunne ikke åpne filen:\n{err}"))?;
        Ok(Self {
            path: path.to_path_buf(),
            size,
            offset: 0,
            window: String::new(),
            prepared_for: None,
        })
    }

    pub fn status(&self) -> String {
        let percent = if self.size == 0 {
            0
        } else {
            ((self.offset as f64 / self.size as f64) * 100.0).round() as u64
        };
        format!(
            "{} · {} % · visning (skrivebeskyttet)",
            format_size(self.size),
            percent.min(100)
        )
    }

    pub fn show(&mut self, ui: &mut egui::Ui, dialog_busy: bool) {
        let row_height = ui.text_style_height(&TextStyle::Monospace);
        let rows = ((ui.available_height() / row_height).floor() as usize).max(1);

        if let Err(message) = self.ensure_window(rows) {
            ui.colored_label(Color32::from_rgb(160, 40, 40), message);
            return;
        }

        if !dialog_busy {
            self.handle_input(ui, rows, row_height, ui.rect_contains_pointer(ui.max_rect()));
        }

        ui.horizontal(|ui| {
            let bar_w = 14.0;
            let text_w = (ui.available_width() - bar_w).max(40.0);
            let height = ui.available_height();

            ui.allocate_ui_with_layout(
                egui::vec2(text_w, height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    ui.add(
                        egui::Label::new(RichText::new(&self.window).monospace())
                            .selectable(true),
                    );
                },
            );

            if let Some(new_offset) = file_scrollbar(ui, self.offset, self.size, height) {
                self.offset = new_offset;
                self.prepared_for = None;
                let _ = self.ensure_window(rows);
            }
        });
    }

    fn handle_input(&mut self, ui: &mut egui::Ui, rows: usize, row_height: f32, hovered: bool) {
        if hovered {
            let scroll_y = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll_y.abs() >= 1.0 {
                let lines = (scroll_y / row_height).round() as i64;
                if lines != 0 {
                    let _ = self.scroll_lines(-lines, rows);
                }
            }
        }

        if ui.input(|input| {
            input.key_pressed(egui::Key::PageDown)
                || (input.key_pressed(egui::Key::ArrowDown) && input.modifiers.command)
        }) {
            let _ = self.scroll_lines(rows as i64, rows);
        } else if ui.input(|input| {
            input.key_pressed(egui::Key::PageUp)
                || (input.key_pressed(egui::Key::ArrowUp) && input.modifiers.command)
        }) {
            let _ = self.scroll_lines(-(rows as i64), rows);
        } else if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            let _ = self.scroll_lines(1, rows);
        } else if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            let _ = self.scroll_lines(-1, rows);
        } else if ui.input(|input| input.key_pressed(egui::Key::Home)) {
            self.offset = 0;
            self.prepared_for = None;
            let _ = self.ensure_window(rows);
        } else if ui.input(|input| input.key_pressed(egui::Key::End)) {
            let _ = self.scroll_to_end(rows);
        }
    }

    fn ensure_window(&mut self, rows: usize) -> Result<(), String> {
        if self.prepared_for == Some((self.offset, rows)) {
            return Ok(());
        }
        let mut file = File::open(&self.path)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.size = file
            .metadata()
            .map(|meta| meta.len())
            .unwrap_or(self.size);
        self.offset = align_line_start(&mut file, self.offset, self.size)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.window = read_window(&mut file, self.offset, self.size, rows)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.prepared_for = Some((self.offset, rows));
        Ok(())
    }

    fn scroll_lines(&mut self, delta: i64, rows: usize) -> Result<(), String> {
        if delta == 0 {
            return Ok(());
        }
        let mut file = File::open(&self.path)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.offset = move_by_lines(&mut file, self.offset, self.size, delta)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.prepared_for = None;
        self.ensure_window(rows)
    }

    fn scroll_to_end(&mut self, rows: usize) -> Result<(), String> {
        let mut file = File::open(&self.path)
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.offset = move_by_lines(&mut file, self.size, self.size, -(rows as i64))
            .map_err(|err| format!("Kunne ikke lese filen:\n{err}"))?;
        self.prepared_for = None;
        self.ensure_window(rows)
    }
}

fn file_scrollbar(ui: &mut egui::Ui, offset: u64, size: u64, height: f32) -> Option<u64> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, height), Sense::click_and_drag());
    let visuals = ui.visuals();
    ui.painter()
        .rect_filled(rect, 3.0, visuals.extreme_bg_color);

    let t = if size == 0 {
        0.0
    } else {
        (offset as f64 / size as f64).clamp(0.0, 1.0) as f32
    };
    let thumb_h = (rect.height() * 0.08).clamp(20.0, rect.height());
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

fn align_line_start(file: &mut File, offset: u64, size: u64) -> std::io::Result<u64> {
    let offset = offset.min(size);
    if offset == 0 {
        return Ok(0);
    }
    let start = offset.saturating_sub(LOOKBACK);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; (offset - start) as usize];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
        Ok(start + i as u64 + 1)
    } else if start == 0 {
        Ok(0)
    } else {
        utf8_floor(file, offset)
    }
}

fn utf8_floor(file: &mut File, offset: u64) -> std::io::Result<u64> {
    let back = offset.min(3);
    file.seek(SeekFrom::Start(offset - back))?;
    let mut buf = [0u8; 3];
    let n = file.read(&mut buf[..back as usize])?;
    let mut i = n;
    while i > 0 && buf[i - 1] & 0xC0 == 0x80 {
        i -= 1;
    }
    Ok(offset - (n - i) as u64)
}

fn read_window(file: &mut File, start: u64, size: u64, rows: usize) -> std::io::Result<String> {
    if start >= size {
        return Ok(String::new());
    }
    let want = MAX_WINDOW.min((size - start) as usize);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; want];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    skip_incomplete_tail(&mut buf);

    let lossy = String::from_utf8_lossy(&buf);
    let mut out = String::new();
    for (index, line) in lossy.split_inclusive('\n').enumerate() {
        if index >= rows {
            break;
        }
        push_display_line(&mut out, line);
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

fn push_display_line(out: &mut String, line: &str) {
    let newline = line.ends_with('\n');
    let body = line.strip_suffix('\n').unwrap_or(line);
    let body = body.strip_suffix('\r').unwrap_or(body);
    let mut chars = body.chars();
    for _ in 0..MAX_LINE_CHARS {
        match chars.next() {
            Some(ch) => out.push(ch),
            None => {
                if newline {
                    out.push('\n');
                }
                return;
            }
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    if newline {
        out.push('\n');
    }
}

fn move_by_lines(file: &mut File, pos: u64, size: u64, delta: i64) -> std::io::Result<u64> {
    if delta > 0 {
        scan_forward(file, pos, size, delta as u64)
    } else if delta < 0 {
        scan_backward(file, pos, (-delta) as u64)
    } else {
        Ok(pos.min(size))
    }
}

fn scan_forward(file: &mut File, mut pos: u64, size: u64, mut lines: u64) -> std::io::Result<u64> {
    let origin = pos;
    file.seek(SeekFrom::Start(pos))?;
    let mut buf = [0u8; CHUNK];
    while lines > 0 && pos < size {
        if pos - origin >= MAX_WINDOW as u64 {
            return Ok(origin.saturating_add(MAX_WINDOW as u64).min(size));
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for (i, byte) in buf[..n].iter().enumerate() {
            if *byte == b'\n' {
                lines -= 1;
                if lines == 0 {
                    return Ok((pos + i as u64 + 1).min(size));
                }
            }
        }
        pos += n as u64;
    }
    Ok(pos.min(size))
}

fn scan_backward(file: &mut File, pos: u64, mut lines: u64) -> std::io::Result<u64> {
    if lines == 0 {
        return Ok(pos);
    }
    if pos == 0 {
        return Ok(0);
    }
    // Search [0, pos-1) so a newline that already ends the previous line
    // (when `pos` is a line start) is not counted as a step.
    let origin = pos;
    let mut end = pos - 1;
    while lines > 0 && end > 0 {
        if origin - end >= MAX_WINDOW as u64 {
            return Ok(origin.saturating_sub(MAX_WINDOW as u64));
        }
        let start = end.saturating_sub(CHUNK as u64);
        let len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; len];
        let n = file.read(&mut buf)?;
        buf.truncate(n);
        for (i, byte) in buf.iter().enumerate().rev() {
            if *byte == b'\n' {
                lines -= 1;
                if lines == 0 {
                    return Ok(start + i as u64 + 1);
                }
            }
        }
        if start == 0 {
            return Ok(0);
        }
        end = start;
    }
    Ok(0)
}

pub fn format_size(bytes: u64) -> String {
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
    format!("{value:.1} {unit}").replace('.', ",")
}

pub const LARGE_SAVE_ERROR: &str =
    "Filen er for stor til å redigeres i RavnPad. Visningen er skrivebeskyttet.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn small_files_open_for_edit() {
        let path = temp("small.txt");
        write_all(&path, b"abc\n");
        match open(&path).unwrap() {
            Opened::Edit(text) => assert_eq!(text, "abc\n"),
            Opened::View(_) => panic!("expected edit"),
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
            Opened::Edit(_) => panic!("expected view"),
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn align_snaps_to_previous_newline() {
        let path = temp("align.txt");
        write_all(&path, b"aaa\nbbbb\ncc");
        let mut file = File::open(&path).unwrap();
        let size = 11;
        assert_eq!(align_line_start(&mut file, 0, size).unwrap(), 0);
        assert_eq!(align_line_start(&mut file, 5, size).unwrap(), 4);
        assert_eq!(align_line_start(&mut file, 10, size).unwrap(), 9);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn move_forward_and_back_over_lines() {
        let path = temp("move.txt");
        write_all(&path, b"one\ntwo\nthree\n");
        let mut file = File::open(&path).unwrap();
        let size = 14;
        let two = scan_forward(&mut file, 0, size, 1).unwrap();
        assert_eq!(two, 4);
        let three = scan_forward(&mut file, two, size, 1).unwrap();
        assert_eq!(three, 8);
        assert_eq!(scan_backward(&mut file, three, 1).unwrap(), 4);
        assert_eq!(scan_backward(&mut file, two, 1).unwrap(), 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn window_stops_after_requested_rows() {
        let path = temp("window.txt");
        write_all(&path, b"a\nb\nc\nd\n");
        let mut file = File::open(&path).unwrap();
        let text = read_window(&mut file, 0, 8, 2).unwrap();
        assert_eq!(text, "a\nb\n");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn long_lines_are_truncated_in_the_window() {
        let path = temp("long.txt");
        let mut data = vec![b'z'; MAX_LINE_CHARS + 50];
        data.push(b'\n');
        write_all(&path, &data);
        let mut file = File::open(&path).unwrap();
        let text = read_window(&mut file, 0, data.len() as u64, 1).unwrap();
        assert!(text.ends_with("…\n"));
        assert_eq!(text.chars().count(), MAX_LINE_CHARS + 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn scan_without_newlines_caps_distance() {
        let path = temp("nonewline.txt");
        let data = vec![b'x'; MAX_WINDOW + CHUNK];
        write_all(&path, &data);
        let mut file = File::open(&path).unwrap();
        let size = data.len() as u64;
        let pos = scan_forward(&mut file, 0, size, 1).unwrap();
        assert_eq!(pos, MAX_WINDOW as u64);
        let back = scan_backward(&mut file, size, 1).unwrap();
        assert_eq!(size - back, MAX_WINDOW as u64);
        let _ = fs::remove_file(path);
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

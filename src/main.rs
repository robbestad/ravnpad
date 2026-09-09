#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;

use eframe::egui::{
    self, containers::scroll_area::ScrollSource, Align, Align2, Color32, FontFamily, FontId, Key,
    KeyboardShortcut, Layout, Modifiers, TextStyle, ViewportCommand,
};
use egui_file_dialog::{DialogState, FileDialog};

#[cfg(windows)]
mod associate;
mod i18n;
mod large;

const APP_NAME: &str = "RavnPad";

const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);

fn main() -> eframe::Result {
    #[cfg(windows)]
    associate::register();

    let lang = i18n::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("ravnpad")
            .with_icon(load_icon())
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([400.0, 280.0])
            .with_drag_and_drop(true)
            .with_fullscreen(false)
            .with_title(format!("{} - {APP_NAME}", lang.text().untitled)),
        persist_window: false,
        renderer: eframe::Renderer::Glow,
        vsync: true,
        // winit has no file-drop events on Wayland. Use X11 (XWayland) so drag-and-drop
        // and window-manager fullscreen behave like a normal desktop app.
        #[cfg(all(unix, not(target_os = "macos")))]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11 as _;
            builder.with_x11();
        })),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(RavnPad::new(cc, initial_path(), lang)))),
    )
}

fn initial_path() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::args_os().nth(1)?);
    path.exists().then_some(path)
}

fn load_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ravnpad-icon.png"
    )))
    .expect("RavnPad-ikon")
}

#[derive(Clone)]
enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Quit,
    OpenPath(PathBuf),
    OpenBytes(String),
}

enum DialogKind {
    Open,
    Save { then: Option<Action> },
}

enum AppError {
    File(large::FileError),
    Save(std::io::Error),
    TooLargeToEdit,
    DropTooLarge,
}

struct RavnPad {
    text: String,
    saved_text: String,
    path: Option<PathBuf>,
    large: Option<large::LargeView>,
    lang: i18n::Lang,
    last_title: String,
    last_size: egui::Vec2,
    file_dialog: FileDialog,
    dialog_kind: Option<DialogKind>,
    confirm: Option<Action>,
    error: Option<AppError>,
}

impl RavnPad {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>, lang: i18n::Lang) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(14.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(15.0, FontFamily::Monospace),
        );
        cc.egui_ctx.set_style(style);
        cc.egui_ctx.set_visuals(egui::Visuals::light());

        let mut app = Self {
            text: String::new(),
            saved_text: String::new(),
            path: None,
            large: None,
            lang,
            last_title: String::new(),
            last_size: egui::Vec2::ZERO,
            file_dialog: FileDialog::new()
                .default_file_name(lang.text().untitled_file)
                .labels(lang.text().file_dialog())
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0]),
            dialog_kind: None,
            confirm: None,
            error: None,
        };

        if let Some(path) = initial {
            app.open_path(path);
        }

        app
    }

    fn t(&self) -> &'static i18n::UiText {
        self.lang.text()
    }

    fn set_lang(&mut self, lang: i18n::Lang) {
        if self.lang == lang {
            return;
        }
        self.lang = lang;
        i18n::save(lang);
        let t = lang.text();
        self.file_dialog.config_mut().labels = t.file_dialog();
        if self.path.is_none() {
            self.file_dialog.config_mut().default_file_name = t.untitled_file.to_owned();
        }
        self.last_title.clear();
    }

    fn is_dirty(&self) -> bool {
        self.text != self.saved_text
    }

    fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.t().untitled.to_owned())
    }

    fn title(&self) -> String {
        let mark = if self.is_dirty() { "*" } else { "" };
        format!("{}{mark} - {APP_NAME}", self.display_name())
    }

    fn request(&mut self, action: Action) {
        if matches!(
            action,
            Action::New | Action::Open | Action::Quit | Action::OpenPath(_) | Action::OpenBytes(_)
        ) && self.is_dirty()
        {
            self.confirm = Some(action);
            return;
        }
        self.execute(action);
    }

    fn execute(&mut self, action: Action) {
        match action {
            Action::New => {
                self.text.clear();
                self.saved_text.clear();
                self.path = None;
                self.large = None;
            }
            Action::Open => {
                self.dialog_kind = Some(DialogKind::Open);
                self.file_dialog.pick_file();
            }
            Action::Save => {
                if self.large.is_some() {
                    self.error = Some(AppError::TooLargeToEdit);
                } else if self.path.is_some() {
                    self.write_current();
                } else {
                    self.start_save(None);
                }
            }
            Action::SaveAs => {
                if self.large.is_some() {
                    self.error = Some(AppError::TooLargeToEdit);
                } else {
                    self.start_save(None);
                }
            }
            Action::Quit => {
                // Close is sent from the caller that has Context.
            }
            Action::OpenPath(path) => self.open_path(path),
            Action::OpenBytes(text) => {
                self.large = None;
                self.text = text;
                self.saved_text.clone_from(&self.text);
                self.path = None;
            }
        }
    }

    fn start_save(&mut self, then: Option<Action>) {
        self.file_dialog.config_mut().default_file_name = self.display_name_for_save();
        self.dialog_kind = Some(DialogKind::Save { then });
        self.file_dialog.save_file();
    }

    fn display_name_for_save(&self) -> String {
        match &self.path {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.t().untitled_file.to_owned()),
            None => self.t().untitled_file.to_owned(),
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        match large::open(&path) {
            Ok(large::Opened::Edit(text)) => {
                self.large = None;
                self.text = text;
                self.saved_text.clone_from(&self.text);
                self.path = Some(path);
            }
            Ok(large::Opened::View(view)) => {
                self.text.clear();
                self.saved_text.clear();
                self.large = Some(view);
                self.path = Some(path);
            }
            Err(err) => self.error = Some(AppError::File(err)),
        }
    }

    fn write_current(&mut self) -> bool {
        if self.large.is_some() {
            self.error = Some(AppError::TooLargeToEdit);
            return false;
        }
        let Some(path) = &self.path else {
            return false;
        };
        match save_text(path, &self.text) {
            Ok(()) => {
                self.saved_text.clone_from(&self.text);
                true
            }
            Err(err) => {
                self.error = Some(AppError::Save(err));
                false
            }
        }
    }

    fn handle_shortcuts(&self, ctx: &egui::Context) -> Option<Action> {
        ctx.input_mut(|input| {
            if input.consume_shortcut(&SAVE_AS) {
                Some(Action::SaveAs)
            } else if input.consume_shortcut(&SAVE) {
                Some(Action::Save)
            } else if input.consume_shortcut(&OPEN) {
                Some(Action::Open)
            } else if input.consume_shortcut(&NEW) {
                Some(Action::New)
            } else if input.consume_shortcut(&QUIT) {
                Some(Action::Quit)
            } else {
                None
            }
        })
    }

    fn take_drops(&mut self, ctx: &egui::Context) -> Option<Action> {
        let files = ctx.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        let file = files.into_iter().next()?;
        if let Some(path) = file.path {
            Some(Action::OpenPath(path))
        } else if let Some(bytes) = file.bytes {
            if bytes.len() as u64 > large::EDIT_LIMIT {
                self.error = Some(AppError::DropTooLarge);
                None
            } else {
                match String::from_utf8(bytes.to_vec()) {
                    Ok(text) => Some(Action::OpenBytes(text)),
                    Err(_) => {
                        self.error = Some(AppError::File(large::FileError::InvalidUtf8));
                        None
                    }
                }
            }
        } else {
            None
        }
    }
}

impl eframe::App for RavnPad {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        avoid_broken_fullscreen(ctx);
        repaint_on_resize(ctx, &mut self.last_size);

        let mut action = self.handle_shortcuts(ctx);
        if action.is_none() {
            action = self.take_drops(ctx);
        }

        let t = self.t();
        egui::TopBottomPanel::top("meny").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(t.file_menu, |ui| {
                    if menu_item(ui, t.new, "Ctrl+N") {
                        action = Some(Action::New);
                    }
                    if menu_item(ui, t.open, "Ctrl+O") {
                        action = Some(Action::Open);
                    }
                    ui.separator();
                    ui.add_enabled_ui(self.large.is_none(), |ui| {
                        if menu_item(ui, t.save, "Ctrl+S") {
                            action = Some(Action::Save);
                        }
                        if menu_item(ui, t.save_as, "Ctrl+Shift+S") {
                            action = Some(Action::SaveAs);
                        }
                    });
                    ui.separator();
                    if menu_item(ui, t.quit, "Ctrl+Q") {
                        action = Some(Action::Quit);
                    }
                });
                ui.menu_button(t.language_menu, |ui| {
                    for &lang in i18n::Lang::ALL {
                        if ui
                            .selectable_label(self.lang == lang, lang.native_name())
                            .clicked()
                        {
                            self.set_lang(lang);
                            ui.close();
                        }
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let path_label = self
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| t.untitled.to_owned());
                ui.label(path_label);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(view) = &self.large {
                        ui.label(view.status(t.view_readonly, t.decimal));
                    } else {
                        ui.label(t.chars(self.text.chars().count()));
                    }
                });
            });
        });

        let dialog_busy = matches!(self.file_dialog.state(), DialogState::Open)
            || self.confirm.is_some()
            || self.error.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(view) = &mut self.large {
                view.show(ui, dialog_busy, t.cannot_read);
            } else {
                let available = ui.available_size();
                let row_height = ui.text_style_height(&TextStyle::Monospace);
                let min_rows = ((available.y / row_height).floor() as usize).max(1);

                egui::ScrollArea::vertical()
                    .id_salt("editor_scroll")
                    .auto_shrink([false, false])
                    .scroll_source(if dialog_busy {
                        ScrollSource::NONE
                    } else {
                        ScrollSource {
                            scroll_bar: true,
                            drag: false,
                            mouse_wheel: true,
                        }
                    })
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.text)
                                .id(egui::Id::new("editor"))
                                .font(TextStyle::Monospace)
                                .desired_width(ui.available_width())
                                .desired_rows(min_rows)
                                .lock_focus(!dialog_busy)
                                .interactive(!dialog_busy),
                        );
                    });
            }
        });

        preview_drop(ctx, t.drop_to_open);
        fit_file_dialog(&mut self.file_dialog, ctx);
        self.file_dialog.update(ctx);

        if matches!(self.file_dialog.state(), DialogState::Cancelled) {
            self.dialog_kind = None;
        }

        if let Some(path) = self.file_dialog.take_picked() {
            match self.dialog_kind.take() {
                Some(DialogKind::Open) | None => self.open_path(path),
                Some(DialogKind::Save { then }) => {
                    self.path = Some(path);
                    if self.write_current()
                        && let Some(then) = then
                    {
                        if matches!(then, Action::Quit) {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        } else {
                            self.execute(then);
                        }
                    }
                }
            }
        }

        if let Some(pending) = self.confirm.clone() {
            let mut decision = None;
            egui::Window::new(t.unsaved_title)
                .collapsible(false)
                .resizable(false)
                .constrain(true)
                .max_size(dialog_max_size(ctx))
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(t.unsaved(&self.display_name()));
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(t.save).clicked() {
                            decision = Some(ConfirmChoice::Save);
                        }
                        if ui.button(t.dont_save).clicked() {
                            decision = Some(ConfirmChoice::Discard);
                        }
                        if ui.button(t.cancel).clicked() {
                            decision = Some(ConfirmChoice::Cancel);
                        }
                    });
                });

            if let Some(choice) = decision {
                self.confirm = None;
                match choice {
                    ConfirmChoice::Save => {
                        if self.path.is_some() {
                            if self.write_current() {
                                if matches!(pending, Action::Quit) {
                                    ctx.send_viewport_cmd(ViewportCommand::Close);
                                } else {
                                    self.execute(pending);
                                }
                            }
                        } else {
                            self.start_save(Some(pending));
                        }
                    }
                    ConfirmChoice::Discard => {
                        if matches!(pending, Action::Quit) {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        } else {
                            self.execute(pending);
                        }
                    }
                    ConfirmChoice::Cancel => {}
                }
            }
        }

        if let Some(error) = &self.error {
            let message = match error {
                AppError::File(err) => t.file_error(err),
                AppError::Save(err) => t.save_error(err),
                AppError::TooLargeToEdit => t.too_large_edit.to_owned(),
                AppError::DropTooLarge => t.drop_too_large.to_owned(),
            };
            let mut close = false;
            egui::Window::new(t.error_title)
                .collapsible(false)
                .resizable(false)
                .constrain(true)
                .max_size(dialog_max_size(ctx))
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(message);
                    ui.add_space(8.0);
                    if ui.button(t.ok).clicked() {
                        close = true;
                    }
                });
            if close {
                self.error = None;
            }
        }

        if ctx.input(|input| input.viewport().close_requested()) && self.is_dirty() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            action = Some(Action::Quit);
        }

        if let Some(action) = action {
            if matches!(action, Action::Quit) && !self.is_dirty() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else if matches!(action, Action::Quit) {
                self.request(Action::Quit);
            } else {
                self.request(action);
            }
        }

        let title = self.title();
        if self.last_title != title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

enum ConfirmChoice {
    Save,
    Discard,
    Cancel,
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui
        .add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked
}

#[cfg(test)]
fn load_text(path: &std::path::Path) -> Result<String, String> {
    match large::open(path) {
        Ok(large::Opened::Edit(text)) => Ok(text),
        Ok(large::Opened::View(_)) => Err("too large".to_owned()),
        Err(large::FileError::InvalidUtf8) => Err("UTF-8".to_owned()),
        Err(err) => Err(format!("{err:?}")),
    }
}

fn save_text(path: &std::path::Path, text: &str) -> Result<(), std::io::Error> {
    fs::write(path, text.as_bytes())
}

fn dialog_max_size(ctx: &egui::Context) -> egui::Vec2 {
    let size = ctx.content_rect().size();
    const PAD: f32 = 16.0;
    egui::vec2((size.x - PAD).max(120.0), (size.y - PAD).max(100.0))
}

fn fit_file_dialog(dialog: &mut FileDialog, ctx: &egui::Context) {
    let max = dialog_max_size(ctx);
    let cfg = dialog.config_mut();
    cfg.max_size = Some(max);
    cfg.min_size = egui::vec2(max.x.min(340.0), max.y.min(170.0));
    cfg.default_size = egui::vec2(max.x.min(650.0), max.y.min(370.0));
    cfg.anchor = Some((Align2::CENTER_CENTER, egui::Vec2::ZERO));
    cfg.show_left_panel = max.x >= 460.0;
}

fn preview_drop(ctx: &egui::Context, message: &str) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop_overlay"),
    ));
    let rect = ctx.content_rect();
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(140));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        message,
        FontId::proportional(22.0),
        Color32::WHITE,
    );
}

fn repaint_on_resize(ctx: &egui::Context, last_size: &mut egui::Vec2) {
    let size = ctx.content_rect().size();
    if *last_size != size {
        *last_size = size;
        ctx.request_repaint();
    }
}

fn avoid_broken_fullscreen(ctx: &egui::Context) {
    if !cfg!(target_os = "linux") {
        return;
    }
    if ctx.input(|input| input.viewport().fullscreen == Some(true)) {
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::load_text;
    use std::io::Write;

    #[test]
    fn load_utf8_text() {
        let dir = std::env::temp_dir();
        let path = dir.join("ravnpad-utf8-test.txt");
        fs_write(&path, "hei\nverden");
        assert_eq!(load_text(&path).unwrap(), "hei\nverden");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reject_invalid_utf8() {
        let dir = std::env::temp_dir();
        let path = dir.join("ravnpad-binary-test.bin");
        fs_write_bytes(&path, &[0xff, 0xfe, 0x00]);
        let err = load_text(&path).unwrap_err();
        assert!(err.contains("UTF-8"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("ravnpad-roundtrip.txt");
        super::save_text(&path, "linje 1\nlinje 2").unwrap();
        assert_eq!(load_text(&path).unwrap(), "linje 1\nlinje 2");
        let _ = std::fs::remove_file(path);
    }

    fn fs_write(path: &std::path::Path, text: &str) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn fs_write_bytes(path: &std::path::Path, bytes: &[u8]) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }
}

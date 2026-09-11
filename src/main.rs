#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, KeyboardShortcut, Layout, Modifiers, PointerButton,
    TextStyle, ViewportCommand,
    containers::scroll_area::ScrollSource,
    style::ScrollAnimation,
    text::{CCursor, CCursorRange},
};

#[cfg(windows)]
mod associate;
mod fonts;
mod i18n;
mod large;
mod native_dialog;
mod prefs;
mod rtf;
mod spell;
mod update;

const APP_NAME: &str = "RavnPad";

const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);

fn main() -> eframe::Result {
    #[cfg(windows)]
    associate::register();
    update::cleanup_old();

    let prefs = prefs::Prefs::load();
    let lang = prefs.lang;
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
        Box::new(move |cc| Ok(Box::new(RavnPad::new(cc, initial_path(), prefs)))),
    )
}

fn initial_path() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from)
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
    CheckUpdate,
    InstallUpdate { url: String },
}

enum AppError {
    File(large::FileError),
    Save(std::io::Error),
    Settings(std::io::Error),
    Dictionary(String),
    DictUnavailable,
    TooLargeToEdit,
    DropTooLarge,
}

enum SpellState {
    Off,
    Loading,
    Ready(spellbook::Dictionary),
}

enum SpellEvent {
    Loaded(spellbook::Dictionary),
    Failed(spell::SpellError),
}

struct RavnPad {
    text: String,
    saved_text: String,
    path: Option<PathBuf>,
    large: Option<large::LargeView>,
    prefs: prefs::Prefs,
    fonts: Vec<fonts::FontChoice>,
    settings_open: bool,
    last_title: String,
    last_size: egui::Vec2,
    confirm: Option<Action>,
    error: Option<AppError>,
    update_tx: Sender<UpdateEvent>,
    update_rx: Receiver<UpdateEvent>,
    update: UpdateUi,
    restarting: bool,
    find_open: bool,
    find_query: String,
    replace_with: String,
    find_index: usize,
    find_focus: bool,
    pending_goto: Option<FindMatch>,
    spell: SpellState,
    spell_misses: Vec<spell::Miss>,
    spell_dirty: bool,
    spell_menu: Option<FindMatch>,
    spell_tx: Sender<SpellEvent>,
    spell_rx: Receiver<SpellEvent>,
}

#[derive(Clone, Copy)]
struct FindMatch {
    byte: usize,  // byte offset of the match, for replace_range
    cchar: usize, // char index of the match, for the egui cursor range
    len: usize,   // match length in chars
}

impl RavnPad {
    fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        prefs: prefs::Prefs,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let font_list = fonts::available_fonts();
        fonts::apply(&cc.egui_ctx, &prefs.font, prefs.size, &font_list);
        let (update_tx, update_rx) = mpsc::channel();
        let (spell_tx, spell_rx) = mpsc::channel();

        let mut app = Self {
            text: String::new(),
            saved_text: String::new(),
            path: None,
            large: None,
            prefs,
            fonts: font_list,
            settings_open: false,
            last_title: String::new(),
            last_size: egui::Vec2::ZERO,
            confirm: None,
            error: None,
            update_tx,
            update_rx,
            update: UpdateUi::Idle,
            restarting: false,
            find_open: false,
            find_query: String::new(),
            replace_with: String::new(),
            find_index: 0,
            find_focus: false,
            pending_goto: None,
            spell: SpellState::Off,
            spell_misses: Vec::new(),
            spell_dirty: false,
            spell_menu: None,
            spell_tx,
            spell_rx,
        };

        if !cfg!(debug_assertions) {
            app.spawn_update_check(false);
        }

        if app.prefs.spellcheck {
            app.start_spellcheck();
        }

        if let Some(path) = initial {
            app.open_path(path);
        }

        app
    }

    fn t(&self) -> &'static i18n::UiText {
        self.prefs.lang.text()
    }

    fn set_lang(&mut self, lang: i18n::Lang) {
        if self.prefs.lang == lang {
            return;
        }
        self.prefs.lang = lang;
        self.save_prefs();
        self.last_title.clear();
        if self.prefs.spellcheck {
            self.stop_spellcheck();
            self.start_spellcheck();
        }
    }

    fn apply_editor_font(&mut self, ctx: &egui::Context) {
        fonts::apply(ctx, &self.prefs.font, self.prefs.size, &self.fonts);
        self.save_prefs();
    }

    fn save_prefs(&mut self) {
        if let Err(err) = self.prefs.save() {
            self.error = Some(AppError::Settings(err));
        }
    }

    fn goto_match(&mut self, ctx: &egui::Context, m: FindMatch) {
        self.pending_goto = Some(m);
        let editor = egui::Id::new("editor");
        let mut st = egui::text_edit::TextEditState::load(ctx, editor).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(m.cchar),
            CCursor::new(m.cchar + m.len),
        )));
        st.store(ctx, editor);
    }

    fn step_match(&mut self, ctx: &egui::Context, matches: &[FindMatch], step: i64) {
        if matches.is_empty() {
            return;
        }
        let n = matches.len() as i64;
        self.find_index = (self.find_index as i64 + step).rem_euclid(n) as usize;
        let m = matches[self.find_index];
        self.goto_match(ctx, m);
    }

    fn close_find(&mut self, ctx: &egui::Context) {
        self.find_open = false;
        ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("editor")));
    }

    fn start_spellcheck(&mut self) {
        self.spell = SpellState::Loading;
        let lang = self.prefs.lang;
        let tx = self.spell_tx.clone();
        thread::spawn(move || {
            let event = match spell::load(lang) {
                Ok(dict) => SpellEvent::Loaded(dict),
                Err(err) => SpellEvent::Failed(err),
            };
            let _ = tx.send(event);
        });
    }

    fn stop_spellcheck(&mut self) {
        self.spell = SpellState::Off;
        self.spell_misses.clear();
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(t.settings_menu);
            ui.add_space(10.0);

            ui.label(t.language_menu);
            let mut lang = self.prefs.lang;
            egui::ComboBox::from_id_salt("settings_lang")
                .selected_text(lang.native_name())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for &choice in i18n::Lang::ALL {
                        ui.selectable_value(&mut lang, choice, choice.native_name());
                    }
                });
            if lang != self.prefs.lang {
                self.set_lang(lang);
            }

            ui.add_space(10.0);
            ui.label(t.font_label);
            let mut font = self.prefs.font.clone();
            let current_name = self
                .fonts
                .iter()
                .find(|choice| choice.id == font)
                .map(|choice| {
                    if choice.id.is_empty() {
                        t.font_default
                    } else {
                        choice.name.as_str()
                    }
                })
                .unwrap_or(t.font_default);
            egui::ComboBox::from_id_salt("settings_font")
                .selected_text(current_name)
                .width(ui.available_width())
                .height(220.0)
                .show_ui(ui, |ui| {
                    for choice in &self.fonts {
                        let label = if choice.id.is_empty() {
                            t.font_default
                        } else {
                            choice.name.as_str()
                        };
                        ui.selectable_value(&mut font, choice.id.clone(), label);
                    }
                });
            if font != self.prefs.font {
                self.prefs.font = font;
                self.apply_editor_font(ctx);
            }

            ui.add_space(10.0);
            ui.label(t.font_size_label);
            let mut size = self.prefs.size;
            if ui
                .add(egui::Slider::new(&mut size, 10.0..=36.0).integer())
                .changed()
            {
                self.prefs.size = size;
                self.apply_editor_font(ctx);
            }

            ui.add_space(10.0);
            let mut spell_on = !matches!(self.spell, SpellState::Off);
            if ui.checkbox(&mut spell_on, t.spellcheck).changed() {
                self.prefs.spellcheck = spell_on;
                self.save_prefs();
                if spell_on {
                    self.start_spellcheck();
                } else {
                    self.stop_spellcheck();
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.weak(format!("RavnPad {}", update::CURRENT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(t.ok).clicked() {
                        close = true;
                    }
                });
            });
        });
        if close || modal.should_close() {
            self.settings_open = false;
        }
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
            Action::New
                | Action::Open
                | Action::Quit
                | Action::OpenPath(_)
                | Action::OpenBytes(_)
                | Action::InstallUpdate { .. }
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
                self.spell_dirty = true;
            }
            Action::Open => {
                if let Some(path) = self.pick_open_path() {
                    self.open_path(path);
                }
            }
            Action::Save => {
                if self.large.is_some() {
                    self.error = Some(AppError::TooLargeToEdit);
                } else if self.path.is_some() {
                    self.write_current();
                } else {
                    self.start_save();
                }
            }
            Action::SaveAs => {
                if self.large.is_some() {
                    self.error = Some(AppError::TooLargeToEdit);
                } else {
                    self.start_save();
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
                self.spell_dirty = true;
            }
            Action::CheckUpdate => {
                self.update = UpdateUi::Checking { user: true };
                self.spawn_update_check(true);
            }
            Action::InstallUpdate { url } => self.spawn_install(url),
        }
    }

    fn spawn_update_check(&self, user: bool) {
        let tx = self.update_tx.clone();
        thread::spawn(move || {
            let event = if update::asset_name().is_none() {
                user.then_some(UpdateEvent::Unsupported)
            } else {
                match update::check_latest() {
                    Ok(update::Check::Available { version, url }) => {
                        Some(UpdateEvent::Available { version, url })
                    }
                    Ok(update::Check::UpToDate) if user => Some(UpdateEvent::UpToDate),
                    Err(err) if user => Some(UpdateEvent::Failed(err.to_string())),
                    _ => None,
                }
            };
            if let Some(event) = event {
                let _ = tx.send(event);
            }
        });
    }

    fn spawn_install(&mut self, url: String) {
        self.update = UpdateUi::Downloading;
        let tx = self.update_tx.clone();
        thread::spawn(move || {
            let event = match update::install(&url) {
                Ok(restart) => UpdateEvent::Ready(restart),
                Err(err) => UpdateEvent::Failed(err.to_string()),
            };
            let _ = tx.send(event);
        });
    }

    fn poll_spell(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.spell_rx.try_recv() {
            match event {
                SpellEvent::Loaded(dict) => {
                    if self.prefs.spellcheck {
                        self.spell = SpellState::Ready(dict);
                        self.spell_dirty = true;
                    }
                }
                SpellEvent::Failed(err) => {
                    self.spell = SpellState::Off;
                    self.error = Some(match err {
                        spell::SpellError::Unavailable => AppError::DictUnavailable,
                        other => AppError::Dictionary(other.to_string()),
                    });
                }
            }
            ctx.request_repaint();
        }
    }

    fn poll_update(&mut self, ctx: &egui::Context) -> Option<Action> {
        while let Ok(event) = self.update_rx.try_recv() {
            match event {
                UpdateEvent::Available { version, url } => {
                    self.update = UpdateUi::Available { version, url };
                }
                UpdateEvent::UpToDate => {
                    self.update = if matches!(self.update, UpdateUi::Checking { user: true }) {
                        UpdateUi::UpToDate
                    } else {
                        UpdateUi::Idle
                    };
                }
                UpdateEvent::Failed(message) => {
                    let show = matches!(
                        self.update,
                        UpdateUi::Checking { user: true } | UpdateUi::Downloading
                    );
                    self.update = if show {
                        UpdateUi::Failed(message)
                    } else {
                        UpdateUi::Idle
                    };
                }
                UpdateEvent::Unsupported => {
                    self.update = if matches!(self.update, UpdateUi::Checking { user: true }) {
                        UpdateUi::Unsupported
                    } else {
                        UpdateUi::Idle
                    };
                }
                UpdateEvent::Ready(restart) => {
                    let launched = match restart {
                        update::Restart::Spawn(exe) => {
                            let mut cmd = std::process::Command::new(&exe);
                            cmd.args(std::env::args_os().skip(1));
                            match cmd.spawn() {
                                Ok(_) => true,
                                Err(err) => {
                                    self.update = UpdateUi::Failed(err.to_string());
                                    false
                                }
                            }
                        }
                        #[cfg(target_os = "macos")]
                        update::Restart::Helper => true,
                    };
                    if launched {
                        self.restarting = true;
                        self.update = UpdateUi::Idle;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
        }

        if !matches!(self.update, UpdateUi::Idle) {
            ctx.request_repaint();
        }

        self.show_update_window(ctx)
    }

    fn show_update_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let t = self.t();
        let mut action = None;
        let mut close = false;
        match &self.update {
            UpdateUi::Idle => {}
            UpdateUi::Checking { .. } => {
                egui::Window::new(t.help_menu)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(t.update_checking);
                    });
            }
            UpdateUi::Downloading => {
                egui::Window::new(t.update_available_title)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(t.update_downloading);
                    });
            }
            UpdateUi::Available { version, url } => {
                let body = t.update_available(version, update::CURRENT);
                let url = url.clone();
                egui::Window::new(t.update_available_title)
                    .collapsible(false)
                    .resizable(false)
                    .constrain(true)
                    .max_size(dialog_max_size(ctx))
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(body);
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(t.update_now).clicked() {
                                action = Some(Action::InstallUpdate { url: url.clone() });
                            }
                            if ui.button(t.update_later).clicked() {
                                close = true;
                            }
                        });
                    });
            }
            UpdateUi::UpToDate => {
                let body = t.update_uptodate_msg(update::CURRENT);
                egui::Window::new(t.help_menu)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(body);
                        ui.add_space(8.0);
                        if ui.button(t.ok).clicked() {
                            close = true;
                        }
                    });
            }
            UpdateUi::Failed(message) => {
                let message = format!("{}\n{message}", t.update_failed);
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
            }
            UpdateUi::Unsupported => {
                egui::Window::new(t.help_menu)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(t.update_unsupported);
                        ui.add_space(8.0);
                        if ui.button(t.ok).clicked() {
                            close = true;
                        }
                    });
            }
        }
        if close {
            self.update = UpdateUi::Idle;
        }
        action
    }

    fn start_save(&mut self) -> bool {
        let Some(path) = self.pick_save_path() else {
            return false;
        };
        self.path = Some(path);
        self.write_current()
    }

    fn pick_open_path(&self) -> Option<PathBuf> {
        native_dialog::pick_open(self.t().open, self.dialog_dir())
    }

    fn pick_save_path(&self) -> Option<PathBuf> {
        native_dialog::pick_save(
            self.t().save_as,
            self.dialog_dir(),
            &self.display_name_for_save(),
        )
    }

    fn dialog_dir(&self) -> Option<&std::path::Path> {
        self.path.as_deref().and_then(std::path::Path::parent)
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
            Ok(large::Opened::Converted { text, txt_path }) => {
                self.large = None;
                self.text = text;
                self.path = Some(txt_path.clone());
                match save_text(&txt_path, &self.text) {
                    Ok(()) => self.saved_text.clone_from(&self.text),
                    Err(err) => {
                        self.saved_text.clear();
                        self.error = Some(AppError::Save(err));
                    }
                }
            }
            Err(err) => self.error = Some(AppError::File(err)),
        }
        self.spell_dirty = true;
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

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        self.select_all(ctx);
        if self.large.is_none()
            && !self.settings_open
            && self.confirm.is_none()
            && self.error.is_none()
            && ctx.input_mut(|input| input.consume_shortcut(&FIND))
        {
            self.find_open = true;
            self.find_focus = true;
        }
        ctx.input_mut(|input| {
            if input.consume_shortcut(&SAVE_AS) {
                Some(Action::SaveAs)
            } else if input.consume_shortcut(&SAVE) {
                Some(Action::Save)
            } else if input.consume_shortcut(&OPEN) {
                Some(Action::Open)
            } else if input.consume_shortcut(&NEW) {
                Some(Action::New)
            } else if input.consume_shortcut(&QUIT) || input.consume_shortcut(&CLOSE) {
                Some(Action::Quit)
            } else {
                None
            }
        })
    }

    // egui scrolls to the primary cursor (the end of the document) when a key
    // press changes the selection, so Cmd+A would yank the view to the bottom.
    // Apply select-all ourselves instead: the stored range then equals the
    // range the TextEdit computes this frame, so nothing scrolls.
    fn select_all(&self, ctx: &egui::Context) {
        let editor = egui::Id::new("editor");
        if self.large.is_some()
            || self.settings_open
            || self.confirm.is_some()
            || self.error.is_some()
            || !matches!(self.update, UpdateUi::Idle)
            || !ctx.memory(|mem| mem.has_focus(editor))
            || !ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::A))
        {
            return;
        }
        let mut state = egui::text_edit::TextEditState::load(ctx, editor).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(0),
            CCursor::new(self.text.chars().count()),
        )));
        state.store(ctx, editor);
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
            } else if rtf::looks_like_rtf(&bytes) {
                match rtf::to_text(&bytes) {
                    Ok(text) => Some(Action::OpenBytes(text)),
                    Err(()) => {
                        self.error = Some(AppError::File(large::FileError::InvalidRtf));
                        None
                    }
                }
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
                if ui.button(t.settings_menu).clicked() {
                    self.settings_open = true;
                }
                ui.add_enabled_ui(self.large.is_none(), |ui| {
                    if ui.button(t.find).clicked() {
                        self.find_open = true;
                        self.find_focus = true;
                    }
                });
                ui.menu_button(t.help_menu, |ui| {
                    ui.label(format!("RavnPad {}", update::CURRENT));
                    ui.separator();
                    if ui.button(t.update_menu).clicked() {
                        action = Some(Action::CheckUpdate);
                        ui.close();
                    }
                });
            });
        });

        let mut matches = if self.find_open && self.large.is_none() {
            find_matches(&self.text, &self.find_query)
        } else {
            Vec::new()
        };
        self.find_index = self.find_index.min(matches.len().saturating_sub(1));

        if self.find_open && self.large.is_none() {
            let mut close_find = false;
            egui::TopBottomPanel::top("finn").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let query = ui.add(
                        egui::TextEdit::singleline(&mut self.find_query)
                            .id(egui::Id::new("sok_felt"))
                            .hint_text(t.find)
                            .desired_width(180.0),
                    );
                    if std::mem::take(&mut self.find_focus) {
                        query.request_focus();
                        let mut st =
                            egui::text_edit::TextEditState::load(ctx, query.id).unwrap_or_default();
                        st.cursor.set_char_range(Some(CCursorRange::two(
                            CCursor::new(0),
                            CCursor::new(self.find_query.chars().count()),
                        )));
                        st.store(ctx, query.id);
                    }
                    if query.changed() {
                        self.find_index = 0;
                        matches = find_matches(&self.text, &self.find_query);
                        if let Some(m) = matches.first().copied() {
                            self.goto_match(ctx, m);
                        }
                    }
                    if query.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let step = if ui.input(|i| i.modifiers.shift) {
                            -1
                        } else {
                            1
                        };
                        self.step_match(ctx, &matches, step);
                        query.request_focus();
                    }
                    let count = if matches.is_empty() {
                        "0".to_owned()
                    } else {
                        format!("{}/{}", self.find_index + 1, matches.len())
                    };
                    ui.weak(count);
                    if ui.button("↑").clicked() {
                        self.step_match(ctx, &matches, -1);
                    }
                    if ui.button("↓").clicked() {
                        self.step_match(ctx, &matches, 1);
                    }
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.replace_with)
                            .id(egui::Id::new("erstatt_felt"))
                            .hint_text(t.replace)
                            .desired_width(140.0),
                    );
                    if ui.button(t.replace).clicked()
                        && let Some(m) = matches.get(self.find_index).copied()
                    {
                        self.text.replace_range(
                            m.byte..m.byte + self.find_query.len(),
                            &self.replace_with,
                        );
                        self.spell_dirty = true;
                        matches = find_matches(&self.text, &self.find_query);
                        self.find_index = self.find_index.min(matches.len().saturating_sub(1));
                        if let Some(next) = matches.get(self.find_index).copied() {
                            self.goto_match(ctx, next);
                        }
                    }
                    if ui.button(t.replace_all).clicked() && !self.find_query.is_empty() {
                        self.text = self.text.replace(&self.find_query, &self.replace_with);
                        self.spell_dirty = true;
                        matches = find_matches(&self.text, &self.find_query);
                        self.find_index = 0;
                    }
                    if ui.button("✕").clicked() {
                        close_find = true;
                    }
                });
            });
            if close_find {
                self.close_find(ctx);
            }
        }

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
                        ui.label(format!(
                            "{} · {}",
                            t.chars(self.text.chars().count()),
                            t.words(self.text.split_whitespace().count())
                        ));
                    }
                });
            });
        });

        let dialog_busy = self.settings_open
            || self.confirm.is_some()
            || self.error.is_some()
            || !matches!(self.update, UpdateUi::Idle);

        if self.find_open && !dialog_busy && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.close_find(ctx);
        }

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
                        let output = egui::TextEdit::multiline(&mut self.text)
                            .id(egui::Id::new("editor"))
                            .font(TextStyle::Monospace)
                            .desired_width(ui.available_width())
                            .desired_rows(min_rows)
                            .lock_focus(!dialog_busy)
                            .interactive(!dialog_busy)
                            .show(ui);
                        drag_scroll(ui, &output.response);
                        paint_matches(ui, &output, &matches, self.find_index);
                        if let SpellState::Ready(dict) = &self.spell {
                            if output.response.changed() || self.spell_dirty {
                                self.spell_misses = spell::misspellings(&self.text, dict);
                                self.spell_dirty = false;
                            }
                            paint_misses(ui, &output, &self.spell_misses);
                        }
                        if !self.spell_misses.is_empty() {
                            if output.response.secondary_clicked()
                                && let Some(pos) = output.response.interact_pointer_pos()
                            {
                                let cursor = output.galley.cursor_from_pos(pos - output.galley_pos);
                                self.spell_menu = self
                                    .spell_misses
                                    .iter()
                                    .copied()
                                    .find(|m| {
                                        cursor.index >= m.cchar && cursor.index <= m.cchar + m.len
                                    })
                                    .map(|m| FindMatch {
                                        byte: m.byte,
                                        cchar: m.cchar,
                                        len: m.len,
                                    });
                            }
                            if self.spell_menu.is_some() {
                                output.response.context_menu(|ui| {
                                    let Some(miss) = self.spell_menu else {
                                        return;
                                    };
                                    let byte_len: usize = self.text[miss.byte..]
                                        .chars()
                                        .take(miss.len)
                                        .map(char::len_utf8)
                                        .sum();
                                    let Some(word) = self
                                        .text
                                        .get(miss.byte..miss.byte + byte_len)
                                        .map(str::to_owned)
                                    else {
                                        return;
                                    };
                                    if let SpellState::Ready(dict) = &self.spell {
                                        let mut suggestions = Vec::new();
                                        dict.suggest(&word, &mut suggestions);
                                        for suggestion in suggestions.into_iter().take(5) {
                                            if ui.button(&suggestion).clicked() {
                                                self.text.replace_range(
                                                    miss.byte..miss.byte + byte_len,
                                                    &suggestion,
                                                );
                                                self.spell_dirty = true;
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                    }
                                    if ui.button(t.add_to_dict).clicked() {
                                        if let SpellState::Ready(dict) = &mut self.spell {
                                            let _ = dict.add(&word);
                                        }
                                        if let Err(err) = spell::add_personal(&word) {
                                            self.error = Some(AppError::Settings(err));
                                        }
                                        self.spell_dirty = true;
                                        ui.close();
                                    }
                                });
                            }
                        }
                        if let Some(m) = self.pending_goto.take() {
                            let rect = match_rects(&output.galley, m.cchar, m.len)
                                .0
                                .translate(output.galley_pos.to_vec2());
                            ui.scroll_to_rect(rect, Some(Align::Center));
                        }
                    });
            }
        });

        if self.settings_open {
            self.settings_window(ctx);
        }

        preview_drop(ctx, t.drop_to_open);

        if let Some(pending) = self.confirm.take() {
            match native_dialog::unsaved(
                t.unsaved_title,
                &t.unsaved(&self.display_name()),
                t.save,
                t.dont_save,
                t.cancel,
            ) {
                native_dialog::Confirm::Save => {
                    let saved = if self.path.is_some() {
                        self.write_current()
                    } else {
                        self.start_save()
                    };
                    if saved {
                        if matches!(pending, Action::Quit) {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        } else {
                            self.execute(pending);
                        }
                    }
                }
                native_dialog::Confirm::Discard => {
                    if matches!(pending, Action::Quit) {
                        self.saved_text.clone_from(&self.text);
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    } else {
                        self.execute(pending);
                    }
                }
                native_dialog::Confirm::Cancel => {}
            }
        }

        if let Some(error) = self.error.take() {
            let message = match error {
                AppError::File(err) => t.file_error(&err),
                AppError::Save(err) => t.save_error(&err),
                AppError::Settings(err) => t.settings_error(&err),
                AppError::Dictionary(err) => t.dictionary_error(&err),
                AppError::DictUnavailable => t.dict_unavailable.to_owned(),
                AppError::TooLargeToEdit => t.too_large_edit.to_owned(),
                AppError::DropTooLarge => t.drop_too_large.to_owned(),
            };
            native_dialog::error(t.error_title, &message);
        }

        if action.is_none() {
            action = self.poll_update(ctx);
        }
        self.poll_spell(ctx);

        if ctx.input(|input| input.viewport().close_requested())
            && self.is_dirty()
            && !self.restarting
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.confirm.is_none() {
                action = Some(Action::Quit);
            }
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

enum UpdateUi {
    Idle,
    Checking { user: bool },
    Available { version: String, url: String },
    Downloading,
    UpToDate,
    Failed(String),
    Unsupported,
}

enum UpdateEvent {
    UpToDate,
    Available { version: String, url: String },
    Ready(update::Restart),
    Failed(String),
    Unsupported,
}

fn find_matches(text: &str, query: &str) -> Vec<FindMatch> {
    let len = query.chars().count();
    if len == 0 {
        return Vec::new();
    }
    let starts: Vec<usize> = text.match_indices(query).map(|(b, _)| b).collect();
    let mut matches = Vec::with_capacity(starts.len());
    let mut next = starts.iter().peekable();
    for (cchar, (byte, _)) in text.char_indices().enumerate() {
        if next.peek() == Some(&&byte) {
            matches.push(FindMatch { byte, cchar, len });
            next.next();
        }
    }
    matches
}

// The editor's own selection is only painted while it has focus, so while
// the find field is focused we draw match boxes ourselves via the galley.
fn paint_matches(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    matches: &[FindMatch],
    current: usize,
) {
    if matches.is_empty() {
        return;
    }
    let clip = output.text_clip_rect.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip);
    let shift = output.galley_pos.to_vec2();
    for (i, &m) in matches.iter().enumerate() {
        let fill = if i == current {
            Color32::from_rgba_unmultiplied(255, 138, 0, 130)
        } else {
            Color32::from_rgba_unmultiplied(255, 193, 7, 60)
        };
        let (first, second) = match_rects(&output.galley, m.cchar, m.len);
        painter.rect_filled(first.translate(shift), 2.0, fill);
        if let Some(rect) = second {
            painter.rect_filled(rect.translate(shift), 2.0, fill);
        }
    }
}

fn paint_misses(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, misses: &[spell::Miss]) {
    if misses.is_empty() {
        return;
    }
    let clip = output.text_clip_rect.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip);
    let shift = output.galley_pos.to_vec2();
    let color = Color32::from_rgb(220, 40, 40);
    for miss in misses {
        let (first, second) = match_rects(&output.galley, miss.cchar, miss.len);
        squiggle(&painter, first.translate(shift), color);
        if let Some(rect) = second {
            squiggle(&painter, rect.translate(shift), color);
        }
    }
}

fn squiggle(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let mut points = Vec::new();
    let mut x = rect.min.x;
    let high = rect.max.y - 1.0;
    let low = rect.max.y - 3.0;
    points.push(egui::pos2(x, high));
    let mut up = true;
    while x < rect.max.x {
        x = (x + 3.0).min(rect.max.x);
        points.push(egui::pos2(x, if up { low } else { high }));
        up = !up;
    }
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, color)));
}

// A match never spans a newline (the query field is single-line) but can
// cross a soft-wrapped row: return the start-segment plus an optional
// continuation segment, in galley coordinates.
fn match_rects(
    galley: &egui::Galley,
    cchar: usize,
    len: usize,
) -> (egui::Rect, Option<egui::Rect>) {
    let a = galley.pos_from_cursor(CCursor::new(cchar));
    let b = galley.pos_from_cursor(CCursor::new(cchar + len));
    if (a.min.y - b.min.y).abs() < 0.5 {
        (
            egui::Rect::from_min_max(a.min, egui::pos2(b.min.x.max(a.min.x), a.max.y)),
            None,
        )
    } else {
        (
            egui::Rect::from_min_max(a.min, egui::pos2(galley.rect.max.x, a.max.y)),
            Some(egui::Rect::from_min_max(
                egui::pos2(galley.rect.min.x, b.min.y),
                b.max,
            )),
        )
    }
}

// Scroll while drag-selecting past the edge of the scroll area so the
// selection can extend beyond the visible text.
fn drag_scroll(ui: &egui::Ui, editor: &egui::Response) {
    if !editor.dragged_by(PointerButton::Primary) {
        return;
    }
    let Some(pos) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    let clip = ui.clip_rect();
    const MAX_SPEED: f32 = 24.0;
    // Positive delta scrolls up, negative scrolls down.
    let delta = if pos.y > clip.bottom() {
        -(pos.y - clip.bottom()).clamp(4.0, MAX_SPEED)
    } else if pos.y < clip.top() {
        (clip.top() - pos.y).clamp(4.0, MAX_SPEED)
    } else {
        0.0
    };
    if delta != 0.0 {
        ui.scroll_with_delta_animation(egui::vec2(0.0, delta), ScrollAnimation::none());
        ui.ctx().request_repaint();
    }
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
        Ok(large::Opened::Converted { text, .. }) => Ok(text),
        Err(large::FileError::InvalidUtf8) => Err("UTF-8".to_owned()),
        Err(large::FileError::InvalidRtf) => Err("RTF".to_owned()),
        Err(err) => Err(format!("{err:?}")),
    }
}

fn save_text(path: &Path, text: &str) -> Result<(), io::Error> {
    let bytes = text.as_bytes();
    match write_all_path(path, bytes) {
        Ok(()) => Ok(()),
        Err(err) if is_transient_io(&err) => {
            thread::sleep(Duration::from_millis(250));
            write_all_path(path, bytes)
        }
        Err(err) => Err(err),
    }
}

fn write_all_path(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = open_for_save(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    let _ = file.sync_all();
    Ok(())
}

fn open_for_save(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE so SMB/AV
        // scanners do not block overwrite.
        options.share_mode(0x0000_0007);
    }
    options.open(path)
}

fn is_transient_io(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    ) || matches!(
        err.raw_os_error(),
        Some(
            32 |  // ERROR_SHARING_VIOLATION
            33 |  // ERROR_LOCK_VIOLATION
            53 |  // ERROR_BAD_NETPATH
            59 |  // ERROR_UNEXP_NET_ERR
            64 |  // ERROR_NETNAME_DELETED
            121 | // ERROR_SEM_TIMEOUT
            1231 | 1232 | 1236
        )
    )
}

fn dialog_max_size(ctx: &egui::Context) -> egui::Vec2 {
    let size = ctx.content_rect().size();
    const PAD: f32 = 16.0;
    egui::vec2((size.x - PAD).max(120.0), (size.y - PAD).max(100.0))
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

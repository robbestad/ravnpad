#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, KeyboardShortcut, Layout, Modifiers, PointerButton,
    TextStyle, ViewportCommand,
    containers::scroll_area::ScrollSource,
    style::ScrollAnimation,
    text::{CCursor, CCursorRange},
};

mod agent;
#[cfg(windows)]
mod associate;
mod document;
mod fonts;
mod host;
mod i18n;
mod large;
#[cfg(target_os = "macos")]
mod macopen;
#[cfg(any(target_os = "macos", windows))]
mod native;
mod native_dialog;
mod prefs;
mod recovery;
mod rtf;
mod spell;
mod storage;
mod update;

const APP_NAME: &str = "RavnPad";

const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const NEW_WINDOW: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);

fn main() -> eframe::Result {
    #[cfg(any(target_os = "macos", windows))]
    {
        native::run();
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        run_egui()
    }
}

#[allow(dead_code)]
fn run_egui() -> eframe::Result {
    #[cfg(windows)]
    associate::register();
    update::cleanup_old();
    #[cfg(target_os = "macos")]
    macopen::install();

    let prefs = prefs::Prefs::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("ravnpad")
            .with_icon(load_icon())
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([400.0, 280.0])
            .with_drag_and_drop(true)
            .with_fullscreen(false)
            // eframe also uses this initial title for the macOS app menu.
            // Document titles are set separately by RavnPad::update.
            .with_title(APP_NAME),
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
    initial_path_from(std::env::args_os().skip(1))
}

fn initial_path_from(
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Option<PathBuf> {
    let mut args = args.into_iter().map(|arg| arg.as_ref().to_owned());
    while let Some(arg) = args.next() {
        if arg == "--connect" {
            let _ = args.next();
            continue;
        }
        if arg != "--enable-agent" && arg != "--explore-agent" {
            return Some(PathBuf::from(arg));
        }
    }
    None
}

fn requested_instance() -> Option<String> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--connect" {
            return args
                .next()
                .map(|value| value.to_string_lossy().into_owned());
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AgentMode {
    #[default]
    Off,
    Explore,
    Edit,
}

impl AgentMode {
    fn requested_from(args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> Self {
        let mut explore = false;
        let mut edit = false;
        for arg in args {
            explore |= arg.as_ref() == "--explore-agent";
            edit |= arg.as_ref() == "--enable-agent";
        }
        if explore {
            Self::Explore
        } else if edit {
            Self::Edit
        } else {
            Self::Off
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: prefs::ThemePref) {
    ctx.set_theme(match theme {
        prefs::ThemePref::System => egui::ThemePreference::System,
        prefs::ThemePref::Light => egui::ThemePreference::Light,
        prefs::ThemePref::Dark => egui::ThemePreference::Dark,
    });
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
    NewWindow,
    Open,
    Save,
    SaveAs,
    Quit,
    OpenPath(PathBuf),
    Recover(PathBuf),
    OpenBytes(String),
    CheckUpdate,
    InstallUpdate { url: String },
}

enum AppError {
    File(large::FileError),
    Save(std::io::Error),
    Durability(std::io::Error),
    Settings(std::io::Error),
    Dictionary(String),
    DictUnavailable,
    TooLargeToEdit,
    DropTooLarge,
    LaunchWindow(std::io::Error),
    Agent(document::Error),
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

enum FileEvent {
    Open(PathBuf, Result<large::Opened, large::FileError>),
    Save(PathBuf, String, io::Result<storage::SaveOutcome>),
}

struct RavnPad {
    file_tx: Sender<FileEvent>,
    file_rx: Receiver<FileEvent>,
    file_busy: bool,
    recovery: recovery::Recovery,
    recovery_candidates: Vec<recovery::Candidate>,
    recovery_due: Option<std::time::Instant>,
    recovered_from: Option<PathBuf>,
    after_save: Option<Action>,
    ctx: egui::Context,
    document_generation: u64,
    document: document::Document,
    host_client: Option<host::Client>,
    host_synced_text: String,
    host_conflict_pending: bool,
    host_poll_due: std::time::Instant,
    agent: Option<agent::Server>,
    agent_mode: AgentMode,
    agent_requested: AgentMode,
    pending_agent: std::collections::VecDeque<String>,
    close_requested: bool,
    text: String,
    saved_text: String,
    cache_valid: bool,
    dirty: bool,
    converted: bool,
    counts: (usize, usize),
    cached_query: String,
    cached_matches: std::sync::Arc<Vec<FindMatch>>,
    path: Option<PathBuf>,
    suggested_path: Option<PathBuf>,
    large: Option<large::LargeView>,
    prefs: prefs::Prefs,
    editor_scroll_y: f32,
    restore_editor_scroll: bool,
    fonts: Vec<fonts::FontChoice>,
    settings_open: bool,
    agent_help_open: bool,
    last_title: String,
    last_size: egui::Vec2,
    confirm: Option<Action>,
    error: Option<AppError>,
    update_tx: Sender<(u64, UpdateEvent)>,
    update_rx: Receiver<(u64, UpdateEvent)>,
    update: UpdateUi,
    update_generation: u64,
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
    spell_visible: std::ops::Range<usize>,
    spell_menu: Option<FindMatch>,
    spell_generation: u64,
    spell_tx: Sender<(u64, SpellEvent)>,
    spell_rx: Receiver<(u64, SpellEvent)>,
    #[cfg(target_os = "macos")]
    pending_opens: std::collections::VecDeque<PathBuf>,
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
        Self::new_core(cc.egui_ctx.clone(), initial, prefs, true)
    }

    fn new_core(
        ctx: egui::Context,
        initial: Option<PathBuf>,
        prefs: prefs::Prefs,
        legacy: bool,
    ) -> Self {
        let font_list = if legacy {
            fonts::available_fonts()
        } else {
            Vec::new()
        };
        if legacy {
            #[cfg(target_os = "macos")]
            macopen::set_context(&ctx);
            apply_theme(&ctx, prefs.theme);
            fonts::apply(&ctx, &prefs.font, prefs.size, &font_list);
        }
        let (update_tx, update_rx) = mpsc::channel();
        let (spell_tx, spell_rx) = mpsc::channel();
        let (file_tx, file_rx) = mpsc::channel();

        let document = document::Document::new("", large::EDIT_LIMIT as usize);
        let agent_requested = AgentMode::requested_from(std::env::args_os().skip(1));
        let mut app = Self {
            file_tx,
            file_rx,
            file_busy: false,
            after_save: None,
            recovery: recovery::Recovery::start(),
            recovery_candidates: Vec::new(),
            recovery_due: None,
            recovered_from: None,
            ctx: ctx.clone(),
            document_generation: 0,
            document,
            host_client: None,
            host_synced_text: String::new(),
            host_conflict_pending: false,
            host_poll_due: std::time::Instant::now(),
            agent: None,
            agent_mode: AgentMode::Off,
            agent_requested,
            pending_agent: std::collections::VecDeque::new(),
            close_requested: false,
            text: String::new(),
            saved_text: String::new(),
            cache_valid: false,
            dirty: false,
            converted: false,
            counts: (0, 0),
            cached_query: String::new(),
            cached_matches: std::sync::Arc::new(Vec::new()),
            path: None,
            suggested_path: None,
            large: None,
            prefs,
            editor_scroll_y: 0.0,
            restore_editor_scroll: false,
            fonts: font_list,
            settings_open: false,
            agent_help_open: false,
            last_title: String::new(),
            last_size: egui::Vec2::ZERO,
            confirm: None,
            error: None,
            update_tx,
            update_rx,
            update: UpdateUi::Idle,
            update_generation: 0,
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
            spell_visible: 0..0,
            spell_menu: None,
            spell_generation: 0,
            spell_tx,
            spell_rx,
            #[cfg(target_os = "macos")]
            pending_opens: std::collections::VecDeque::new(),
        };

        if !cfg!(debug_assertions) {
            app.spawn_update_check(false);
        }

        if legacy && app.prefs.spellcheck {
            app.start_spellcheck();
        }

        if let Some(instance) = requested_instance() {
            if let Err(error) = app.connect_host(&instance) {
                app.error = Some(AppError::Settings(error));
            }
        } else if let Some(path) = initial {
            app.open_path(path);
        } else {
            #[cfg(not(test))]
            {
                if let Err(error) = app.spawn_host(None) {
                    app.error = Some(AppError::Settings(error));
                }
                if app.agent_requested != AgentMode::Off && app.host_client.is_some() {
                    app.start_agent(app.agent_requested);
                }
            }
            #[cfg(test)]
            if app.agent_requested != AgentMode::Off {
                app.start_agent(app.agent_requested);
            }
        }

        app
    }

    fn error_message(&self, error: AppError) -> String {
        let t = self.t();
        match error {
            AppError::File(err) => t.file_error(&err),
            AppError::Save(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                self.prefs.lang.io_text().conflict.to_owned()
            }
            AppError::Save(err) => t.save_error(&err),
            AppError::Durability(err) => format!(
                "{}\n{err}",
                if matches!(self.prefs.lang, i18n::Lang::Bokmal | i18n::Lang::Nynorsk) {
                    "Filen er lagret, men varig lagring kunne ikke bekreftes."
                } else {
                    "The file was saved, but durable storage could not be confirmed."
                }
            ),
            AppError::Settings(err) => t.settings_error(&err),
            AppError::Dictionary(err) => t.dictionary_error(&err),
            AppError::DictUnavailable => t.dict_unavailable.to_owned(),
            AppError::TooLargeToEdit => t.too_large_edit.to_owned(),
            AppError::DropTooLarge => t.drop_too_large.to_owned(),
            AppError::LaunchWindow(err) => format!("{}:\n{err}", t.cannot_open_window),
            AppError::Agent(err) => format!("Agent suggestion could not be applied:\n{err}"),
        }
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
        #[cfg(not(test))]
        {
            // Another open window may have newer history or reading positions.
            let latest = prefs::Prefs::load();
            self.prefs.recent = latest.recent;
            self.prefs.positions = latest.positions;
            if let Err(err) = self.prefs.save() {
                self.error = Some(AppError::Settings(err));
            }
        }
    }

    fn remember_position(&mut self) -> Option<(PathBuf, f32, u64)> {
        let Some(path) = self.path.clone() else {
            return None;
        };
        let offset = self.large.as_ref().map_or(0, large::LargeView::offset);
        self.prefs.set_position(&path, self.editor_scroll_y, offset);
        Some((path, self.editor_scroll_y, offset))
    }

    fn remember_position_and_save(&mut self) {
        let Some(position) = self.remember_position() else {
            return;
        };
        #[cfg(test)]
        let _ = position;
        #[cfg(not(test))]
        {
            // Update only the shared session state, preserving settings changed elsewhere.
            let (path, scroll, offset) = position;
            let mut latest = prefs::Prefs::load();
            latest.set_position(&path, scroll, offset);
            if let Err(err) = latest.save() {
                self.error = Some(AppError::Settings(err));
            } else {
                self.prefs.recent = latest.recent;
                self.prefs.positions = latest.positions;
            }
        }
    }

    fn record_recent(&mut self, path: &Path) {
        #[cfg(test)]
        self.prefs.add_recent(path);
        #[cfg(not(test))]
        {
            // Start with the latest disk state so concurrent windows do not lose entries.
            let mut latest = prefs::Prefs::load();
            latest.add_recent(path);
            if let Err(err) = latest.save() {
                self.error = Some(AppError::Settings(err));
            } else {
                self.prefs.recent = latest.recent;
                self.prefs.positions = latest.positions;
            }
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
        self.spell_generation += 1;
        let generation = self.spell_generation;
        let ctx = self.ctx.clone();
        self.spell = SpellState::Loading;
        let lang = self.prefs.lang;
        let tx = self.spell_tx.clone();
        thread::spawn(move || {
            let event = match spell::load(lang) {
                Ok(dict) => SpellEvent::Loaded(dict),
                Err(err) => SpellEvent::Failed(err),
            };
            let _ = tx.send((generation, event));
            ctx.request_repaint();
        });
    }

    fn stop_spellcheck(&mut self) {
        self.spell_generation += 1;
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
            ui.label(t.theme);
            let mut theme = self.prefs.theme;
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(match theme {
                    prefs::ThemePref::System => t.theme_system,
                    prefs::ThemePref::Light => t.theme_light,
                    prefs::ThemePref::Dark => t.theme_dark,
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut theme, prefs::ThemePref::System, t.theme_system);
                    ui.selectable_value(&mut theme, prefs::ThemePref::Light, t.theme_light);
                    ui.selectable_value(&mut theme, prefs::ThemePref::Dark, t.theme_dark);
                });
            if theme != self.prefs.theme {
                self.prefs.theme = theme;
                apply_theme(ctx, theme);
                self.save_prefs();
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

            let mut line_wrap = self.prefs.line_wrap;
            if ui.checkbox(&mut line_wrap, t.line_wrap).changed() {
                self.prefs.line_wrap = line_wrap;
                self.restore_editor_scroll = false;
                self.save_prefs();
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

    fn refresh_document(&mut self) {
        if self.host_client.is_some() {
            let changed = !self.host_conflict_pending && self.text != self.host_synced_text;
            let due = std::time::Instant::now() >= self.host_poll_due;
            if changed || due {
                let result = if changed {
                    self.host_client
                        .as_mut()
                        .unwrap()
                        .edit(&self.text)
                        .map(Some)
                } else {
                    self.host_client.as_mut().unwrap().refresh_if_changed()
                };
                match result {
                    Ok(Some(state)) => {
                        let state = state.clone();
                        if !self.host_conflict_pending {
                            if self.text != state.text
                                || self.document.identity() != &state.identity
                            {
                                self.apply_host_state(&state);
                            }
                            if !state.dirty {
                                self.saved_text = state.text.clone();
                            }
                            self.host_synced_text = state.text;
                            self.path = state.path;
                            self.dirty = state.dirty;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if changed && error.kind() == io::ErrorKind::WouldBlock {
                            self.preserve_local_host_text();
                            self.resolve_host_conflict();
                        } else {
                            self.host_connection_failed(error, changed);
                        }
                    }
                }
                self.host_poll_due =
                    std::time::Instant::now() + std::time::Duration::from_millis(100);
            }
            self.ctx.request_repaint_after(
                self.host_poll_due
                    .saturating_duration_since(std::time::Instant::now()),
            );
        }
        if !self.cache_valid {
            if self.host_client.is_none() {
                self.document.observe_text(&self.text);
            }
            if self.host_client.is_none() {
                self.dirty = self.converted || self.text != self.saved_text;
            }
            if self.host_client.is_none() && self.dirty {
                if self.recovery_due.is_none() {
                    self.recovery_due =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                    self.ctx
                        .request_repaint_after(std::time::Duration::from_secs(2));
                }
            } else if self.host_client.is_none() {
                self.clear_recovery();
            } else if self.host_conflict_pending {
                let _ = self
                    .recovery
                    .tx
                    .send(recovery::Command::Snapshot(Some(self.text.clone())));
            }
            self.counts = (
                self.text.chars().count(),
                self.text.split_whitespace().count(),
            );
        }
        if !self.cache_valid || self.cached_query != self.find_query {
            self.cached_matches = std::sync::Arc::new(find_matches(&self.text, &self.find_query));
            self.cached_query.clone_from(&self.find_query);
        }
        self.cache_valid = true;
    }

    fn apply_host_state(&mut self, state: &host::HostState) {
        self.host_conflict_pending = false;
        if !state.read_only {
            self.large = None;
        }
        self.text.clone_from(&state.text);
        self.host_synced_text.clone_from(&state.text);
        if !state.dirty {
            self.saved_text.clone_from(&state.text);
        }
        self.path.clone_from(&state.path);
        self.dirty = state.dirty;
        self.document.adopt(state.identity.clone(), &state.text);
        self.agent_mode = match state.agent_mode.as_str() {
            "edit" => AgentMode::Edit,
            "explore" => AgentMode::Explore,
            _ => AgentMode::Off,
        };
        self.cache_valid = false;
        self.spell_dirty = true;
    }

    fn preserve_local_host_text(&mut self) {
        self.host_conflict_pending = true;
        let _ = self
            .recovery
            .tx
            .send(recovery::Command::Snapshot(Some(self.text.clone())));
    }

    fn host_connection_failed(&mut self, error: io::Error, local_edit: bool) {
        if local_edit || self.is_dirty() {
            self.preserve_local_host_text();
        }
        if let Some(client) = self.host_client.take() {
            client.abandon();
        }
        self.agent_mode = AgentMode::Off;
        self.cache_valid = false;
        self.error = Some(AppError::Settings(error));
    }

    /// A stale edit must never replace text that has only been typed locally.
    /// The user explicitly chooses which buffer wins before sending another edit.
    fn resolve_host_conflict(&mut self) -> bool {
        let Some(client) = self.host_client.as_mut() else {
            return false;
        };
        let remote = match client.refresh() {
            Ok(state) => state.clone(),
            Err(error) => {
                self.error = Some(AppError::Settings(error));
                return false;
            }
        };
        match native_dialog::unsaved(
            "Document changed",
            "The host changed while you were editing. Choose which text to keep.",
            "Keep my edits",
            "Use host text",
            "Decide later",
        ) {
            native_dialog::Confirm::Save => {
                let result = self.host_client.as_mut().unwrap().edit(&self.text);
                match result {
                    Ok(state) => {
                        let state = state.clone();
                        self.apply_host_state(&state);
                        let _ = self.recovery.tx.send(recovery::Command::Snapshot(None));
                        true
                    }
                    Err(error) => {
                        self.error = Some(AppError::Settings(error));
                        false
                    }
                }
            }
            native_dialog::Confirm::Discard => {
                self.apply_host_state(&remote);
                let _ = self.recovery.tx.send(recovery::Command::Snapshot(None));
                true
            }
            native_dialog::Confirm::Cancel => false,
        }
    }

    fn connect_host(&mut self, instance: &str) -> io::Result<()> {
        let client = host::Client::connect(instance)?;
        let state = client.state.clone();
        let view = self.host_read_only_view(&state)?;
        self.host_client = Some(client);
        self.apply_host_state(&state);
        self.large = view;
        self.document_generation += 1;
        Ok(())
    }

    fn host_read_only_view(&self, state: &host::HostState) -> io::Result<Option<large::LargeView>> {
        if !state.read_only {
            return Ok(None);
        }
        let path = state
            .path
            .as_deref()
            .ok_or_else(|| io::Error::other("read-only host has no file path"))?;
        let mut view = large::LargeView::reopen(path)?;
        let (_, offset) = self.prefs.position(path);
        view.set_offset(offset);
        Ok(Some(view))
    }

    fn spawn_host(&mut self, path: Option<&Path>) -> io::Result<()> {
        // The previous document keeps running in its own host. Clear its GUI
        // attachment before any failure can leave this buffer tied to it.
        self.host_client.take();
        self.host_synced_text.clear();
        self.host_conflict_pending = false;
        self.agent_mode = AgentMode::Off;
        let client = host::Client::spawn(path, host::AgentMode::Off)?;
        let state = client.state.clone();
        self.host_client = Some(client);
        self.apply_host_state(&state);
        self.document_generation += 1;
        Ok(())
    }

    fn seed_host_text(&mut self, text: &str) {
        let result = self
            .host_client
            .as_mut()
            .map(|client| client.edit(text).map(|state| state.clone()));
        match result {
            Some(Ok(state)) => self.apply_host_state(&state),
            Some(Err(error)) => self.error = Some(AppError::Settings(error)),
            None => {}
        }
    }

    fn host_history(&mut self, undo: bool) {
        if self.host_conflict_pending && !self.resolve_host_conflict() {
            return;
        }
        let Some(client) = self.host_client.as_mut() else {
            return;
        };
        let result = (|| -> io::Result<host::HostState> {
            if client.state.text != self.text {
                client.edit(&self.text)?;
            }
            if undo {
                client.undo()?;
            } else {
                client.redo()?;
            }
            Ok(client.state.clone())
        })();
        match result {
            Ok(state) => self.apply_host_state(&state),
            Err(error) => self.error = Some(AppError::Settings(error)),
        }
    }

    fn start_agent(&mut self, mode: AgentMode) {
        debug_assert!(mode != AgentMode::Off);
        self.agent_requested = AgentMode::Off;
        if self.host_client.is_some() {
            self.set_agent_mode(mode);
            return;
        }
        if !cfg!(test) {
            self.error = Some(AppError::Settings(io::Error::other(
                "document host unavailable",
            )));
            return;
        }
        if self.agent.is_some() {
            self.set_agent_mode(mode);
            return;
        }
        let ctx = self.ctx.clone();
        match agent::Server::start(
            &self.document.identity().instance_id,
            std::sync::Arc::new(move || ctx.request_repaint()),
        ) {
            Ok(server) => {
                self.agent = Some(server);
                self.agent_mode = mode;
            }
            Err(error) => self.error = Some(AppError::Settings(error)),
        }
    }

    fn set_agent_mode(&mut self, mode: AgentMode) {
        if mode == self.agent_mode {
            return;
        }
        if let Some(client) = self.host_client.as_mut() {
            let host_mode = match mode {
                AgentMode::Off => host::AgentMode::Off,
                AgentMode::Explore => host::AgentMode::Explore,
                AgentMode::Edit => host::AgentMode::Edit,
            };
            match client.set_mode(host_mode) {
                Ok(()) => {
                    self.agent_mode = mode;
                    self.agent_requested = AgentMode::Off;
                }
                Err(error) => self.error = Some(AppError::Settings(error)),
            }
            return;
        }
        if mode == AgentMode::Off {
            self.pending_agent.clear();
            self.document.reject_pending();
            self.agent = None;
        } else if self.agent_mode == AgentMode::Off {
            self.start_agent(mode);
            return;
        } else if self.agent_mode == AgentMode::Edit && mode == AgentMode::Explore {
            self.pending_agent.clear();
            self.document.reject_pending();
        }
        self.agent_mode = mode;
        self.agent_requested = AgentMode::Off;
    }

    fn agent_helper_path(name: &str) -> String {
        let executable = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let current = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ravnpad"));
        #[cfg(target_os = "macos")]
        let directory = current
            .parent()
            .and_then(Path::parent)
            .map(|contents| contents.join("Helpers"));
        #[cfg(not(target_os = "macos"))]
        let directory = current.parent().map(Path::to_path_buf);
        directory
            .unwrap_or_default()
            .join(executable)
            .display()
            .to_string()
    }

    fn agent_help_text(&self) -> String {
        let identity = self.document.identity();
        let directory = prefs::config_dir()
            .map(|path| path.join("agent").display().to_string())
            .unwrap_or_else(|| "<agent configuration directory>".to_owned());
        let norwegian = matches!(self.prefs.lang, i18n::Lang::Bokmal | i18n::Lang::Nynorsk);
        let cli = Self::agent_helper_path("ravnpad-cli");
        let mcp = Self::agent_helper_path("ravnpad-mcp");
        let state = match (norwegian, self.agent_mode) {
            (true, AgentMode::Explore) => "UTFORSK — agenten kan lese, men ikke endre dokumentet",
            (true, AgentMode::Edit) => "REDIGER — agenten kan lese og sende validerte endringer",
            (true, AgentMode::Off) => "AV — vinduet eksponerer ikke dokumentet",
            (false, AgentMode::Explore) => {
                "EXPLORE — the agent can read but cannot change the document"
            }
            (false, AgentMode::Edit) => "EDIT — the agent can read and submit validated changes",
            (false, AgentMode::Off) => "OFF — this window does not expose a document endpoint",
        };
        let commands = format!(
            "\"{cli}\" document status --instance {} --json\n\"{cli}\" document read --instance {} --document {} --json\n\"{cli}\" document propose --instance {} --document {} --stdin --json\n\"{cli}\" document save --instance {} --document {} --json",
            identity.instance_id,
            identity.instance_id,
            identity.document_id,
            identity.instance_id,
            identity.document_id,
            identity.instance_id,
            identity.document_id,
        );
        if norwegian {
            format!(
                "Status: {state}\n\nBruk Agent → Utforsk for lesetilgang, Agent → Rediger for å gi endringstilgang, og Agent → Av for å trekke tilbake all tilgang. Agenten kan ikke endre modus selv.\n\nSlik finner agenten vinduet\nRavnPad skriver en eierbeskyttet endpoint-fil i:\n{directory}\n\nFilnavnet er instance-ID-en. Agenten finner filen lokalt og bruker deretter document status for å få ID-en til dokumentet som er åpent nå.\n\nInstance-ID:\n{}\n\nGjeldende document-ID:\n{}\n\nCLI-eksempel\n{commands}\n\nForslaget til document propose sendes som patch-JSON på stdin. Propose og save krever Rediger-modus. MCP-klienter kan starte:\n\"{mcp}\"\nDette tilbyr status, read, propose og save over stdio.\n\nBare lokale prosesser under din brukerkonto får tilgang. Når agenttilgangen slås av, fjernes endpointen. Verten fortsetter etter at vinduet lukkes, til eksplisitt stopp.",
                identity.instance_id, identity.document_id,
            )
        } else {
            format!(
                "Status: {state}\n\nUse Agent → Explore for read access, Agent → Edit to grant change access, and Agent → Off to revoke all access. The agent cannot change this mode itself.\n\nHow an agent finds this window\nRavnPad writes an owner-only endpoint file in:\n{directory}\n\nThe filename is the instance ID. The agent discovers it locally, then uses document status to resolve the ID of the document that is open now.\n\nInstance ID:\n{}\n\nCurrent document ID:\n{}\n\nCLI example\n{commands}\n\nDocument propose receives patch JSON on stdin. Propose and save require Edit mode. MCP clients can start:\n\"{mcp}\"\nThis exposes status, read, propose, and save over stdio.\n\nOnly local processes running as your user can connect. Turning agent access off removes the endpoint. The host continues after the window closes until explicitly stopped.",
                identity.instance_id, identity.document_id,
            )
        }
    }

    fn agent_connection_text(&self) -> String {
        let identity = self.document.identity();
        let directory = prefs::config_dir()
            .map(|path| path.join("agent").display().to_string())
            .unwrap_or_else(|| "<agent configuration directory>".to_owned());
        let cli = Self::agent_helper_path("ravnpad-cli");
        let mcp = Self::agent_helper_path("ravnpad-mcp");
        let norwegian = matches!(self.prefs.lang, i18n::Lang::Bokmal | i18n::Lang::Nynorsk);
        if norwegian {
            let state = match self.agent_mode {
                AgentMode::Explore => "UTFORSK — agenten kan lese, men ikke foreslå endringer",
                AgentMode::Edit => "REDIGER — agenten kan lese og foreslå endringer",
                AgentMode::Off => "AV — velg Utforsk eller Rediger i Agent-menyen først",
            };
            format!(
                "Koble til dette åpne RavnPad-dokumentet.\n\nAgentmodus: {state}\nEndpoint-katalog: {directory}\nInstance-ID: {}\nDocument-ID: {}\n\nCLI:\n\"{cli}\" document status --instance {} --json\n\"{cli}\" document read --instance {} --document {} --json\n\"{cli}\" document propose --instance {} --document {} --stdin --json\n\nMCP-server:\n\"{mcp}\"\n\nBruk document propose for å foreslå endringer; ikke skriv direkte til dokumentfilen.",
                identity.instance_id,
                identity.document_id,
                identity.instance_id,
                identity.instance_id,
                identity.document_id,
                identity.instance_id,
                identity.document_id,
            )
        } else {
            let state = match self.agent_mode {
                AgentMode::Explore => "EXPLORE — the agent can read but cannot propose changes",
                AgentMode::Edit => "EDIT — the agent can read and propose changes",
                AgentMode::Off => "OFF — select Explore or Edit from the Agent menu first",
            };
            format!(
                "Connect to this open RavnPad document.\n\nAgent mode: {state}\nEndpoint directory: {directory}\nInstance ID: {}\nDocument ID: {}\n\nCLI:\n\"{cli}\" document status --instance {} --json\n\"{cli}\" document read --instance {} --document {} --json\n\"{cli}\" document propose --instance {} --document {} --stdin --json\n\nMCP server:\n\"{mcp}\"\n\nUse document propose to suggest changes; do not write directly to the document file.",
                identity.instance_id,
                identity.document_id,
                identity.instance_id,
                identity.instance_id,
                identity.document_id,
                identity.instance_id,
                identity.document_id,
            )
        }
    }

    fn replace_document(&mut self) {
        self.pending_agent.clear();
        self.document.replace_document(&self.text);
    }

    fn require_agent_edit_access(&self) -> Result<(), agent::ApiError> {
        if self.agent_mode == AgentMode::Edit {
            Ok(())
        } else {
            Err(agent::ApiError::new(
                "read_only",
                "agent access only permits reading; choose Agent > Edit in RavnPad to allow changes",
            ))
        }
    }

    fn poll_agent(&mut self, edits_allowed: bool) -> Option<document::Proposal> {
        let mut applied_proposal = None;
        loop {
            let request = match self.agent.as_ref().map(agent::Server::try_recv) {
                Some(Ok(request)) => request,
                _ => break,
            };
            if !self
                .agent
                .as_ref()
                .is_some_and(|server| server.authorizes(&request.request))
            {
                request.respond(agent::Response::Error {
                    error: agent::ApiError::new("unauthorized", "invalid or revoked access token"),
                });
                continue;
            }
            match &request.request {
                agent::Request::DocumentStatus { .. } => {
                    request.respond(agent::Response::Document {
                        document: self.document.identity().clone(),
                    });
                }
                agent::Request::DocumentRead {
                    document_id,
                    offset,
                    limit,
                    ..
                } => {
                    if self.large.is_some() {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "content_unavailable",
                                "large document content is not exposed through this API version",
                            ),
                        });
                        continue;
                    }
                    if document_id != &self.document.identity().document_id {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "wrong_document",
                                "document is no longer open",
                            ),
                        });
                        continue;
                    }
                    let start = offset.unwrap_or(0);
                    let end = limit
                        .map(|limit| start.saturating_add(limit).min(self.text.len()))
                        .unwrap_or(self.text.len());
                    if start > self.text.len()
                        || !self.text.is_char_boundary(start)
                        || !self.text.is_char_boundary(end)
                    {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "invalid_range",
                                "read range is not on UTF-8 boundaries",
                            ),
                        });
                        continue;
                    }
                    if limit.is_none() && self.text.len() > 512 * 1024 {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "range_required",
                                "documents over 512 KiB require offset and limit",
                            ),
                        });
                        continue;
                    }
                    let complete = start == 0 && end == self.text.len();
                    let read_only = self.large.is_some() || self.text.contains('\0');
                    let snapshot = if complete {
                        self.document.snapshot(
                            &self.text,
                            self.path.as_ref().map(|_| self.saved_text.as_str()),
                            self.is_dirty(),
                            document::ExternalState::Unknown,
                            read_only,
                            true,
                            self.agent_mode == AgentMode::Edit,
                        )
                    } else {
                        self.document.ranged_snapshot(
                            &self.text[start..end],
                            self.is_dirty(),
                            document::ExternalState::Unknown,
                            read_only,
                        )
                    };
                    request.respond(agent::bounded_snapshot_response(snapshot));
                }
                agent::Request::DocumentPropose { patch, .. } => {
                    if let Err(error) = self.require_agent_edit_access() {
                        request.respond(agent::Response::Error { error });
                        continue;
                    }
                    if !edits_allowed
                        || self.file_busy
                        || matches!(self.update, UpdateUi::Downloading)
                        || self.restarting
                        || self.close_requested
                        || self.confirm.is_some()
                    {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "busy",
                                "a file operation is in progress; reread and retry when it finishes",
                            ),
                        });
                        continue;
                    }
                    if self.large.is_some() || self.text.contains('\0') {
                        request.respond(agent::Response::Error {
                            error: agent::ApiError::new(
                                "read_only",
                                "this document cannot be edited",
                            ),
                        });
                        continue;
                    }
                    #[cfg(any(target_os = "macos", windows))]
                    let proposed = self.document.propose_with_line_endings(
                        patch.clone(),
                        &self.text,
                        self.native_uses_crlf(),
                    );
                    #[cfg(not(any(target_os = "macos", windows)))]
                    let proposed = self.document.propose(patch.clone(), &self.text);
                    match proposed {
                        Ok(proposal) => {
                            let proposal = proposal.clone();
                            match self.document.proposal_status(&proposal.operation_id) {
                                Some(document::ProposalStatus::Pending) => {
                                    match self.approve_agent_proposal(&proposal.operation_id) {
                                        Ok(_) => {
                                            self.refresh_document();
                                            request.respond(agent::Response::Applied {
                                                proposal: (&proposal).into(),
                                            });
                                            applied_proposal = Some(proposal);
                                        }
                                        Err(error) => {
                                            self.reject_agent_proposal(&proposal.operation_id);
                                            request.respond(agent::Response::Error {
                                                error: agent::ApiError::document(error),
                                            });
                                        }
                                    }
                                }
                                Some(document::ProposalStatus::Applied) => {
                                    request.respond(agent::Response::Applied {
                                        proposal: (&proposal).into(),
                                    });
                                }
                                Some(document::ProposalStatus::Rejected) => {
                                    request.respond(agent::Response::Rejected {
                                        proposal: (&proposal).into(),
                                    });
                                }
                                None => unreachable!("proposal was just stored"),
                            }
                        }
                        Err(error) => request.respond(agent::Response::Error {
                            error: agent::ApiError::document(error),
                        }),
                    }
                }
                _ => request.respond(agent::Response::Error {
                    error: agent::ApiError::new(
                        "unsupported",
                        "host command unavailable in legacy GUI",
                    ),
                }),
            }
            if applied_proposal.is_some() {
                break;
            }
        }
        applied_proposal
    }

    fn approve_agent_proposal(&mut self, operation_id: &str) -> Result<String, document::Error> {
        if self.agent_mode != AgentMode::Edit {
            self.document.reject_pending();
            return Err(document::Error::ReadOnlyAccess);
        }
        let text = self.document.approve(operation_id, &self.text)?;
        self.text.clone_from(&text);
        self.document.finish_approval(operation_id, &text)?;
        self.cache_valid = false;
        self.spell_dirty = true;
        Ok(text)
    }

    #[cfg(any(target_os = "macos", windows))]
    fn native_uses_crlf(&self) -> bool {
        if self.path.is_some() {
            if self.host_client.is_some() {
                self.text.contains("\r\n")
            } else {
                self.saved_text.contains("\r\n")
            }
        } else {
            self.text.contains("\r\n")
        }
    }

    fn reject_agent_proposal(&mut self, operation_id: &str) {
        let _ = self.document.reject(operation_id);
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.host_conflict_pending
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
                | Action::Recover(_)
                | Action::OpenBytes(_)
                | Action::InstallUpdate { .. }
        ) && self.is_dirty()
            && !(matches!(action, Action::Quit)
                && self.host_client.is_some()
                && !self.host_conflict_pending)
        {
            self.confirm = Some(action);
            return;
        }
        self.execute(action);
    }

    fn execute(&mut self, action: Action) {
        match action {
            Action::New => {
                self.document_generation += 1;
                self.remember_position_and_save();
                self.text.clear();
                self.replace_document();
                self.converted = false;
                self.suggested_path = None;
                self.saved_text.clear();
                self.dirty = false;
                self.path = None;
                self.large = None;
                self.editor_scroll_y = 0.0;
                self.restore_editor_scroll = true;
                self.spell_dirty = true;
                self.cache_valid = false;
                #[cfg(not(test))]
                if let Err(error) = self.spawn_host(None) {
                    self.error = Some(AppError::Settings(error));
                }
            }
            Action::NewWindow => match std::env::current_exe()
                .and_then(|exe| std::process::Command::new(exe).spawn())
            {
                Ok(_) => {}
                Err(err) => self.error = Some(AppError::LaunchWindow(err)),
            },
            Action::Open => {
                if let Some(path) = self.pick_open_path() {
                    self.remember_position_and_save();
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
            Action::OpenPath(path) => {
                self.remember_position_and_save();
                self.open_path(path);
            }
            Action::Recover(path) => {
                self.remember_position_and_save();
                #[cfg(not(test))]
                if self.host_client.is_some() {
                    if let Err(error) = self.spawn_host(None) {
                        self.error = Some(AppError::Settings(error));
                        return;
                    }
                    let result = self
                        .host_client
                        .as_mut()
                        .unwrap()
                        .recover(&path)
                        .map(Clone::clone);
                    match result {
                        Ok(state) => {
                            self.apply_host_state(&state);
                            self.large = None;
                            self.converted = false;
                            self.suggested_path = None;
                            self.recovery_candidates.clear();
                            self.editor_scroll_y = 0.0;
                            self.restore_editor_scroll = true;
                        }
                        Err(error) => self.error = Some(AppError::Settings(error)),
                    }
                    return;
                }
                self.file_busy = true;
                if self
                    .recovery
                    .tx
                    .send(recovery::Command::Read(path))
                    .is_err()
                {
                    self.file_busy = false;
                    self.error = Some(AppError::Settings(io::Error::other("Recovery unavailable")));
                }
            }
            Action::OpenBytes(text) => {
                self.document_generation += 1;
                self.remember_position_and_save();
                self.large = None;
                self.text = text;
                self.replace_document();
                self.converted = false;
                self.suggested_path = None;
                self.saved_text.clone_from(&self.text);
                self.path = None;
                self.editor_scroll_y = 0.0;
                self.restore_editor_scroll = true;
                self.spell_dirty = true;
                self.cache_valid = false;
                #[cfg(not(test))]
                {
                    let text = self.text.clone();
                    if let Err(error) = self.spawn_host(None) {
                        self.error = Some(AppError::Settings(error));
                    }
                    self.seed_host_text(&text);
                }
            }
            Action::CheckUpdate => {
                if matches!(self.update, UpdateUi::Downloading) {
                    return;
                }
                self.update = UpdateUi::Checking { user: true };
                self.spawn_update_check(true);
            }
            Action::InstallUpdate { url } => self.spawn_install(url),
        }
    }

    fn spawn_update_check(&mut self, user: bool) {
        self.update_generation += 1;
        let generation = self.update_generation;
        let ctx = self.ctx.clone();
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
                let _ = tx.send((generation, event));
                ctx.request_repaint();
            }
        });
    }

    fn spawn_install(&mut self, url: String) {
        self.update_generation += 1;
        let generation = self.update_generation;
        let ctx = self.ctx.clone();
        self.update = UpdateUi::Downloading;
        let tx = self.update_tx.clone();
        thread::spawn(move || {
            let event = match update::install(&url) {
                Ok(restart) => UpdateEvent::Ready(restart),
                Err(err) => UpdateEvent::Failed(err.to_string()),
            };
            let _ = tx.send((generation, event));
            ctx.request_repaint();
        });
    }

    fn poll_spell(&mut self, ctx: &egui::Context) {
        while let Ok((generation, event)) = self.spell_rx.try_recv() {
            if generation != self.spell_generation || !self.prefs.spellcheck {
                continue;
            }
            match event {
                SpellEvent::Loaded(dict) => {
                    if self.prefs.spellcheck {
                        self.spell = SpellState::Ready(dict);
                        self.spell_dirty = true;
                        self.cache_valid = false;
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
        self.poll_update_events(ctx);
        self.show_update_window(ctx)
    }

    // Shared state handling must also work without an active egui frame.
    // Native frontends present their own dialogs after processing these events.
    fn poll_update_events(&mut self, ctx: &egui::Context) {
        while let Ok((generation, event)) = self.update_rx.try_recv() {
            if generation != self.update_generation {
                continue;
            }
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
                        self.close_requested = true;
                        self.update = UpdateUi::Idle;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
        }

        if !matches!(self.update, UpdateUi::Idle) {
            ctx.request_repaint();
        }
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
        if !cfg!(test) && self.host_client.is_none() {
            self.error = Some(AppError::Save(io::Error::other(
                "document host unavailable",
            )));
            return false;
        }
        let Some(path) = self.pick_save_path() else {
            return false;
        };
        self.begin_save(path)
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
        self.path
            .as_deref()
            .or(self.suggested_path.as_deref())
            .and_then(std::path::Path::parent)
    }

    fn display_name_for_save(&self) -> String {
        match self.path.as_ref().or(self.suggested_path.as_ref()) {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.t().untitled_file.to_owned()),
            None => self.t().untitled_file.to_owned(),
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.file_busy = true;
        let tx = self.file_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = large::open(&path);
            let _ = tx.send(FileEvent::Open(path, result));
            ctx.request_repaint();
        });
    }

    fn apply_open(&mut self, path: PathBuf, result: Result<large::Opened, large::FileError>) {
        if result.is_ok() {
            self.document_generation += 1;
        }
        let recent_path = path.clone();
        let (scroll_y, large_offset) = self.prefs.position(&path);
        let opened = result.is_ok();
        match result {
            Ok(large::Opened::Edit(text)) => {
                self.converted = false;
                self.suggested_path = None;
                self.large = None;
                self.text = text;
                self.replace_document();
                self.saved_text.clone_from(&self.text);
                self.path = Some(path);
                self.editor_scroll_y = scroll_y;
                self.restore_editor_scroll = true;
            }
            Ok(large::Opened::View(mut view)) => {
                view.set_offset(large_offset);
                self.converted = false;
                self.suggested_path = None;
                self.text.clear();
                self.replace_document();
                self.saved_text.clear();
                self.large = Some(view);
                self.path = Some(path);
                self.editor_scroll_y = 0.0;
                self.restore_editor_scroll = false;
            }
            Ok(large::Opened::Converted { text, txt_path }) => {
                self.large = None;
                self.text = text;
                self.replace_document();
                self.suggested_path = Some(txt_path);
                // Conversion must never write a sibling .txt file just by opening RTF.
                self.path = None;
                self.saved_text.clear();
                self.converted = true;
                self.editor_scroll_y = 0.0;
                self.restore_editor_scroll = true;
            }
            Err(err) => {
                self.error = Some(AppError::File(err));
                // Do not start an agent on an unrelated buffer when the requested
                // document failed to open. Launch without a path for an empty buffer.
                self.agent_requested = AgentMode::Off;
            }
        }
        if opened {
            self.record_recent(&recent_path);
            #[cfg(not(test))]
            {
                let host_path = self.path.clone();
                let seed_text = self.text.clone();
                if let Err(error) = self.spawn_host(host_path.as_deref()) {
                    self.error = Some(AppError::Settings(error));
                }
                if host_path.is_none() {
                    self.seed_host_text(&seed_text);
                }
            }
        }
        if self.agent_requested != AgentMode::Off && self.host_client.is_some() {
            self.start_agent(self.agent_requested);
        }
        self.spell_dirty = true;
        self.cache_valid = false;
    }

    fn write_current(&mut self) -> bool {
        if !cfg!(test) && self.host_client.is_none() {
            self.error = Some(AppError::Save(io::Error::other(
                "document host unavailable",
            )));
            return false;
        }
        if self.large.is_some() {
            self.error = Some(AppError::TooLargeToEdit);
            return false;
        }
        let Some(path) = &self.path else {
            return false;
        };
        self.begin_save(path.clone())
    }

    fn begin_save(&mut self, path: PathBuf) -> bool {
        if self.host_conflict_pending && !self.resolve_host_conflict() {
            return false;
        }
        if let Some(client) = self.host_client.as_mut() {
            self.file_busy = true;
            let text = self.text.clone();
            if client.state.text != self.text
                && let Err(error) = client.edit(&self.text)
            {
                self.file_busy = false;
                if error.kind() == io::ErrorKind::WouldBlock {
                    self.preserve_local_host_text();
                    if self.resolve_host_conflict() {
                        return self.begin_save(path);
                    }
                } else {
                    self.host_connection_failed(error, true);
                }
                return false;
            }
            let result = if client.state.path.as_ref() == Some(&path) {
                client.save()
            } else {
                client.save_as(&path)
            };
            if let Err(error) = &result {
                if error.kind() == io::ErrorKind::WouldBlock {
                    self.file_busy = false;
                    self.preserve_local_host_text();
                    if self.resolve_host_conflict() {
                        return self.begin_save(path);
                    }
                    return false;
                }
                if !matches!(
                    error.kind(),
                    io::ErrorKind::Other | io::ErrorKind::AlreadyExists
                ) {
                    self.file_busy = false;
                    self.host_connection_failed(
                        io::Error::new(error.kind(), error.to_string()),
                        true,
                    );
                    return false;
                }
            }
            let result = result.map(|warning| storage::SaveOutcome {
                durability_warning: warning.map(io::Error::other),
            });
            let _ = self.file_tx.send(FileEvent::Save(path, text, result));
            return true;
        }
        self.file_busy = true;
        let text = self.text.clone();
        let expected = if self.path.as_ref() == Some(&path) {
            Some(self.saved_text.as_bytes().to_vec())
        } else {
            None
        };
        let tx = self.file_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = match expected {
                Some(expected) => storage::save_checked(&path, text.as_bytes(), Some(&expected)),
                // The native Save As dialog confirms replacement of an existing file.
                None => save_text(&path, &text),
            };
            let _ = tx.send(FileEvent::Save(path, text, result));
            ctx.request_repaint();
        });
        true
    }

    fn clear_recovery(&mut self) {
        self.recovery_due = None;
        if let Some(path) = self.recovered_from.take() {
            let _ = self.recovery.tx.send(recovery::Command::Delete(path));
        }
        let _ = self.recovery.tx.send(recovery::Command::Snapshot(None));
    }

    fn poll_recovery(&mut self) {
        while let Ok(event) = self.recovery.rx.try_recv() {
            match event {
                recovery::Event::Available(paths) => self.recovery_candidates = paths,
                recovery::Event::Restored(path, text) => {
                    self.document_generation += 1;
                    self.recovered_from = Some(path);
                    self.suggested_path = None;
                    self.file_busy = false;
                    self.text = text;
                    self.replace_document();
                    self.saved_text.clear();
                    self.path = None;
                    self.large = None;
                    self.converted = true;
                    self.cache_valid = false;
                    self.spell_dirty = true;
                    self.recovery_candidates.clear();
                    #[cfg(not(test))]
                    {
                        let text = self.text.clone();
                        if let Err(error) = self.spawn_host(None) {
                            self.error = Some(AppError::Settings(error));
                        }
                        self.seed_host_text(&text);
                    }
                }
                recovery::Event::Failed(e) => self.error = Some(AppError::Settings(e)),
                recovery::Event::ReadFailed(e) => {
                    self.file_busy = false;
                    self.error = Some(AppError::Settings(e));
                }
            }
        }
        if let Some(due) = self.recovery_due {
            if std::time::Instant::now() >= due {
                let _ = self
                    .recovery
                    .tx
                    .send(recovery::Command::Snapshot(Some(self.text.clone())));
                self.recovery_due = None;
            } else {
                self.ctx.request_repaint_after(
                    due.saturating_duration_since(std::time::Instant::now()),
                );
            }
        }
    }

    fn poll_files(&mut self) {
        while let Ok(event) = self.file_rx.try_recv() {
            self.file_busy = false;
            match event {
                FileEvent::Open(path, result) => self.apply_open(path, result),
                FileEvent::Save(path, text, result) => match result {
                    Ok(outcome) => {
                        self.path = Some(path.clone());
                        self.record_recent(&path);
                        self.converted = false;
                        self.suggested_path = None;
                        self.saved_text = text;
                        self.cache_valid = false;
                        self.refresh_document();
                        if self.host_client.is_some() {
                            self.clear_recovery();
                        }
                        if let Some(warning) = outcome.durability_warning {
                            self.after_save = None;
                            self.error = Some(AppError::Durability(warning));
                            continue;
                        }
                        if let Some(action) = self.after_save.take() {
                            if matches!(action, Action::Quit) {
                                self.close_requested = true;
                                self.ctx.send_viewport_cmd(ViewportCommand::Close);
                            } else {
                                self.execute(action);
                            }
                        }
                    }
                    Err(err) => {
                        self.after_save = None;
                        self.error = Some(AppError::Save(err));
                    }
                },
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        if self.host_client.is_some() && ctx.input_mut(|input| input.consume_shortcut(&UNDO)) {
            self.host_history(true);
            return None;
        }
        if self.host_client.is_some() && ctx.input_mut(|input| input.consume_shortcut(&REDO)) {
            self.host_history(false);
            return None;
        }
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
            } else if input.consume_shortcut(&NEW_WINDOW) {
                Some(Action::NewWindow)
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

    fn remap_editor_selection(ctx: &egui::Context, proposal: &document::Proposal) {
        fn char_position(text: &str, byte: usize) -> usize {
            text[..byte].chars().count()
        }
        fn map_position(position: usize, proposal: &document::Proposal) -> usize {
            let mut delta = 0isize;
            for hunk in &proposal.hunks {
                let start = char_position(&proposal.before, hunk.before_start);
                let end = char_position(&proposal.before, hunk.before_end);
                let after_start = char_position(&proposal.after, hunk.after_start);
                let after_end = char_position(&proposal.after, hunk.after_end);
                if position < start {
                    break;
                }
                if start < end && position < end {
                    return after_start + (position - start).min(after_end - after_start);
                }
                delta += (after_end - after_start) as isize - (end - start) as isize;
            }
            position.saturating_add_signed(delta)
        }

        let editor = egui::Id::new("editor");
        let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor) else {
            return;
        };
        let Some(mut range) = state.cursor.char_range() else {
            return;
        };
        range.primary.index = map_position(range.primary.index, proposal);
        range.secondary.index = map_position(range.secondary.index, proposal);
        range.h_pos = None;
        state.cursor.set_char_range(Some(range));
        state.store(ctx, editor);
    }

    // Files macOS asks us to open (double-click, "Open With"). One per
    // frame so the save-confirmation can gate each open like drops do.
    fn take_macos_opens(&mut self) -> Option<Action> {
        #[cfg(target_os = "macos")]
        {
            if self.confirm.is_some() {
                return None;
            }
            self.pending_opens.extend(macopen::drain());
            if let Some(path) = self.pending_opens.pop_front() {
                return Some(Action::OpenPath(path));
            }
        }
        None
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
        self.poll_recovery();
        self.poll_files();
        self.refresh_document();
        if let Some(proposal) = self.poll_agent(true) {
            Self::remap_editor_selection(ctx, &proposal);
        }
        if self.file_busy {
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
                ui.label(self.prefs.lang.io_text().busy);
            });
            return;
        }
        avoid_broken_fullscreen(ctx);
        repaint_on_resize(ctx, &mut self.last_size);

        let mut action = self.handle_shortcuts(ctx);
        if action.is_none() {
            action = self.take_drops(ctx);
        }
        if action.is_none() {
            action = self.take_macos_opens();
        }

        let t = self.t();
        if let Some(operation_id) = self.pending_agent.front().cloned() {
            let proposal = self.document.proposal(&operation_id).cloned();
            if let Some(proposal) = proposal {
                let mut approve = false;
                let mut reject = false;
                egui::Window::new("Agent suggestion")
                    .collapsible(false)
                    .resizable(true)
                    .show(ctx, |ui| {
                        ui.label(format!("Operation: {}", proposal.operation_id));
                        ui.columns(2, |columns| {
                            columns[0].heading("Before");
                            egui::ScrollArea::vertical().max_height(300.0).show(
                                &mut columns[0],
                                |ui| {
                                    ui.monospace(&proposal.before);
                                },
                            );
                            columns[1].heading("After");
                            egui::ScrollArea::vertical().max_height(300.0).show(
                                &mut columns[1],
                                |ui| {
                                    ui.monospace(&proposal.after);
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            approve = ui.button("Apply").clicked();
                            reject = ui.button("Reject").clicked();
                        });
                    });
                if approve {
                    if let Err(error) = self.approve_agent_proposal(&operation_id) {
                        self.reject_agent_proposal(&operation_id);
                        self.error = Some(AppError::Agent(error));
                    }
                    self.pending_agent.pop_front();
                } else if reject {
                    self.reject_agent_proposal(&operation_id);
                    self.pending_agent.pop_front();
                }
            } else {
                self.pending_agent.pop_front();
            }
        }
        let recent_files = self.prefs.recent.clone();
        let recent_labels = prefs::recent_labels(&recent_files);
        if !self.recovery_candidates.is_empty() {
            let labels = self.prefs.lang.io_text();
            let mut restore = None;
            let mut delete = None;
            let mut recovery_open = true;
            egui::Window::new(labels.recovery)
                .open(&mut recovery_open)
                .show(ctx, |ui| {
                    ui.label(labels.description);
                    for (i, candidate) in self.recovery_candidates.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(&candidate.preview);
                            if ui.button(labels.restore).clicked() {
                                restore = Some(candidate.path.clone());
                            }
                            if ui.button(labels.delete).clicked() {
                                delete = Some(i);
                            }
                        });
                    }
                });
            if !recovery_open {
                self.recovery_candidates.clear();
            }
            if let Some(path) = restore {
                action = Some(Action::Recover(path));
            }
            if let Some(i) = delete {
                let path = self.recovery_candidates.remove(i).path;
                let _ = self.recovery.tx.send(recovery::Command::Delete(path));
            }
        }
        egui::TopBottomPanel::top("meny").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(t.file_menu, |ui| {
                    if menu_item(ui, t.new, "Ctrl+N") {
                        action = Some(Action::New);
                    }
                    if menu_item(ui, t.new_window, "Ctrl+Shift+N") {
                        action = Some(Action::NewWindow);
                    }
                    if menu_item(ui, t.open, "Ctrl+O") {
                        action = Some(Action::Open);
                    }
                    ui.add_enabled_ui(!recent_files.is_empty(), |ui| {
                        ui.menu_button(t.recent_files, |ui| {
                            for (path, label) in recent_files.iter().zip(&recent_labels) {
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    action = Some(Action::OpenPath(path.clone()));
                                    ui.close();
                                }
                            }
                        });
                    });
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
                ui.menu_button(t.agent_menu, |ui| {
                    let mut mode = self.agent_mode;
                    ui.radio_value(&mut mode, AgentMode::Off, self.prefs.lang.agent_off());
                    ui.radio_value(&mut mode, AgentMode::Explore, t.enable_agent);
                    ui.radio_value(&mut mode, AgentMode::Edit, t.agent_enabled);
                    if mode != self.agent_mode {
                        self.set_agent_mode(mode);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(self.prefs.lang.agent_help()).clicked() {
                        self.agent_help_open = true;
                        ui.close();
                    }
                    if ui.button(self.prefs.lang.copy_agent_info()).clicked() {
                        ui.ctx().copy_text(self.agent_connection_text());
                        ui.close();
                    }
                });
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
            self.cached_matches.clone()
        } else {
            std::sync::Arc::new(Vec::new())
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
                        matches = std::sync::Arc::new(find_matches(&self.text, &self.find_query));
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
                        self.cache_valid = false;
                        matches = std::sync::Arc::new(find_matches(&self.text, &self.find_query));
                        self.find_index = self.find_index.min(matches.len().saturating_sub(1));
                        if let Some(next) = matches.get(self.find_index).copied() {
                            self.goto_match(ctx, next);
                        }
                    }
                    if ui.button(t.replace_all).clicked() && !self.find_query.is_empty() {
                        self.text = self.text.replace(&self.find_query, &self.replace_with);
                        self.spell_dirty = true;
                        self.cache_valid = false;
                        matches = std::sync::Arc::new(find_matches(&self.text, &self.find_query));
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
                            t.chars(self.counts.0),
                            t.words(self.counts.1)
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
                view.show(ui, dialog_busy, t.cannot_read, self.prefs.line_wrap);
            } else {
                let available = ui.available_size();
                let row_height = ui.text_style_height(&TextStyle::Monospace);
                let min_rows = ((available.y / row_height).floor() as usize).max(1);

                let line_wrap = self.prefs.line_wrap;
                let mut scroll_area = if line_wrap {
                    egui::ScrollArea::vertical()
                } else {
                    egui::ScrollArea::both()
                }
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
                });
                if std::mem::take(&mut self.restore_editor_scroll) {
                    scroll_area = scroll_area.vertical_scroll_offset(self.editor_scroll_y);
                }
                let scroll_output = scroll_area.show(ui, |ui| {
                    let output = egui::TextEdit::multiline(&mut self.text)
                        .id(egui::Id::new("editor"))
                        .font(TextStyle::Monospace)
                        .desired_width(if line_wrap {
                            ui.available_width()
                        } else {
                            f32::INFINITY
                        })
                        .desired_rows(min_rows)
                        .lock_focus(!dialog_busy)
                        .interactive(!dialog_busy && (cfg!(test) || self.host_client.is_some()))
                        .show(ui);
                    if output.response.changed() {
                        self.cache_valid = false;
                    }
                    drag_scroll(ui, &output.response);
                    paint_matches(ui, &output, &matches, self.find_index);
                    if let SpellState::Ready(dict) = &self.spell {
                        let visible = visible_chars(ui, &output);
                        if output.response.changed()
                            || self.spell_dirty
                            || visible != self.spell_visible
                        {
                            self.spell_misses =
                                spell::misspellings(&self.text, dict, visible.clone());
                            self.spell_visible = visible;
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
                                    for suggestion in
                                        spell::suggest(&word, dict).into_iter().take(5)
                                    {
                                        if ui.button(&suggestion).clicked() {
                                            self.text.replace_range(
                                                miss.byte..miss.byte + byte_len,
                                                &suggestion,
                                            );
                                            self.spell_dirty = true;
                                            self.cache_valid = false;
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
                                    self.cache_valid = false;
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
                self.editor_scroll_y = scroll_output.state.offset.y;
            }
        });

        if self.settings_open {
            self.settings_window(ctx);
        }

        if self.agent_help_open {
            let mut open = self.agent_help_open;
            egui::Window::new(self.prefs.lang.agent_help())
                .open(&mut open)
                .resizable(true)
                .default_width(620.0)
                .show(ctx, |ui| {
                    ui.style_mut().interaction.selectable_labels = true;
                    ui.add(egui::Label::new(self.agent_help_text()).wrap());
                    ui.add_space(12.0);
                    if ui.button(self.prefs.lang.copy_agent_info()).clicked() {
                        ui.ctx().copy_text(self.agent_connection_text());
                    }
                });
            self.agent_help_open = open;
        }

        preview_drop(ctx, t.drop_to_open);

        self.refresh_document();
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
                        self.after_save = Some(pending);
                    }
                }
                native_dialog::Confirm::Discard => {
                    self.clear_recovery();
                    if matches!(pending, Action::Quit) {
                        self.saved_text.clone_from(&self.text);
                        self.dirty = false;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    } else {
                        self.execute(pending);
                    }
                }
                native_dialog::Confirm::Cancel => {}
            }
        }

        if let Some(error) = self.error.take() {
            let message = self.error_message(error);
            native_dialog::error(t.error_title, &message);
        }

        if action.is_none() {
            action = self.poll_update(ctx);
        }
        self.poll_spell(ctx);

        if ctx.input(|input| input.viewport().close_requested())
            && self.is_dirty()
            && (self.host_client.is_none() || self.host_conflict_pending)
            && !self.restarting
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.confirm.is_none() {
                action = Some(Action::Quit);
            }
        }

        if let Some(action) = action {
            if matches!(action, Action::Quit)
                && (!self.is_dirty() || (self.host_client.is_some() && !self.host_conflict_pending))
            {
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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut client) = self.host_client.take() {
            let _ = client.detach();
        }
        self.remember_position_and_save();
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

// Include complete rows at both edges, including partially clipped rows.
fn visible_chars(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
) -> std::ops::Range<usize> {
    let clip = output.text_clip_rect.intersect(ui.clip_rect());
    let top = output
        .galley
        .cursor_from_pos(egui::vec2(-f32::MAX, clip.min.y - output.galley_pos.y))
        .index;
    let bottom = output
        .galley
        .cursor_from_pos(egui::vec2(f32::MAX, clip.max.y - output.galley_pos.y))
        .index;
    top..bottom.saturating_add(1)
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
    let visible = visible_chars(ui, output);
    let start = matches.partition_point(|m| m.cchar + m.len < visible.start);
    for (i, &m) in matches
        .iter()
        .enumerate()
        .skip(start)
        .take_while(|(_, m)| m.cchar < visible.end)
    {
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
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0_f32, color)));
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

fn save_text(path: &Path, text: &str) -> Result<storage::SaveOutcome, io::Error> {
    storage::save_checked(path, text.as_bytes(), None)
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
    use super::*;

    #[test]
    fn reconnect_restores_large_read_only_view() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(&dir.path().join("recovery"));
        let path = dir.path().join("large.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(large::EDIT_LIMIT + 1).unwrap();
        let state = host::HostState {
            identity: document::Document::new("", 1).identity().clone(),
            text: String::new(),
            path: Some(path.clone()),
            dirty: false,
            undo_depth: 0,
            redo_depth: 0,
            agent_mode: "off".into(),
            read_only: true,
        };
        let view = app.host_read_only_view(&state).unwrap().unwrap();
        assert_eq!(view.path, path);
        assert_eq!(view.size, large::EDIT_LIMIT + 1);
    }

    #[test]
    fn lost_host_transport_keeps_local_text_in_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let recovery_dir = dir.path().join("recovery");
        let mut app = test_app(&recovery_dir);
        app.text = "unsent local edit".into();
        app.host_connection_failed(io::Error::from(io::ErrorKind::BrokenPipe), true);
        assert_eq!(app.text, "unsent local edit");
        assert!(app.host_client.is_none());
        assert!(app.is_dirty());
        app.recovery.finish();
        let copies = std::fs::read_dir(&recovery_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
            .collect::<Vec<_>>();
        assert_eq!(copies.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&copies[0]).unwrap(),
            "unsent local edit"
        );
    }

    #[test]
    fn missing_startup_document_is_reported_in_every_agent_mode() {
        for mode in [AgentMode::Off, AgentMode::Explore, AgentMode::Edit] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = test_app(&dir.path().join("recovery"));
            let path = dir.path().join("missing.txt");
            app.agent_requested = mode;
            app.text = "existing unsaved text".into();

            app.apply_open(path.clone(), large::open(&path));

            assert!(matches!(
                app.error,
                Some(AppError::File(large::FileError::Open(ref error)))
                    if error.kind() == io::ErrorKind::NotFound
            ));
            assert_eq!(app.text, "existing unsaved text");
            assert!(app.path.is_none());
            assert!(app.agent.is_none());
            assert_eq!(app.agent_requested, AgentMode::Off);
            assert!(app.prefs.recent.is_empty());
        }
    }

    #[test]
    fn other_startup_open_errors_are_still_reported() {
        for error in [
            large::FileError::Open(io::Error::from(io::ErrorKind::PermissionDenied)),
            large::FileError::InvalidUtf8,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = test_app(&dir.path().join("recovery"));
            app.agent_requested = AgentMode::Edit;

            app.apply_open(dir.path().join("notes.txt"), Err(error));

            assert!(matches!(app.error, Some(AppError::File(_))));
            assert!(app.agent.is_none());
            assert_eq!(app.agent_requested, AgentMode::Off);
        }
    }

    fn test_app(dir: &std::path::Path) -> super::RavnPad {
        let ctx = egui::Context::default();
        let prefs = prefs::Prefs {
            lang: i18n::Lang::English,
            font: String::new(),
            size: 15.0,
            spellcheck: true,
            theme: prefs::ThemePref::System,
            line_wrap: true,
            recent: Vec::new(),
            positions: Vec::new(),
        };
        let (update_tx, update_rx) = mpsc::channel();
        let (spell_tx, spell_rx) = mpsc::channel();
        let (file_tx, file_rx) = mpsc::channel();
        let app = RavnPad {
            file_tx,
            file_rx,
            file_busy: false,
            after_save: None,
            recovery: recovery::Recovery::start_in(Some(dir.to_owned())),
            recovery_candidates: Vec::new(),
            recovery_due: None,
            recovered_from: None,
            ctx: ctx.clone(),
            document_generation: 0,
            document: document::Document::new("", large::EDIT_LIMIT as usize),
            host_client: None,
            host_synced_text: String::new(),
            host_conflict_pending: false,
            host_poll_due: std::time::Instant::now(),
            agent: None,
            agent_mode: AgentMode::Off,
            agent_requested: AgentMode::Off,
            pending_agent: std::collections::VecDeque::new(),
            close_requested: false,
            text: String::new(),
            saved_text: String::new(),
            cache_valid: false,
            dirty: false,
            converted: false,
            counts: (0, 0),
            cached_query: String::new(),
            cached_matches: std::sync::Arc::new(Vec::new()),
            path: None,
            suggested_path: None,
            large: None,
            prefs,
            editor_scroll_y: 0.0,
            restore_editor_scroll: false,
            fonts: Vec::new(),
            settings_open: false,
            agent_help_open: false,
            last_title: String::new(),
            last_size: egui::Vec2::ZERO,
            confirm: None,
            error: None,
            update_tx,
            update_rx,
            update: UpdateUi::Idle,
            update_generation: 0,
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
            spell_visible: 0..0,
            spell_menu: None,
            spell_generation: 0,
            spell_tx,
            spell_rx,
            #[cfg(target_os = "macos")]
            pending_opens: std::collections::VecDeque::new(),
        };
        app
    }

    #[test]
    fn startup_flags_select_access_and_are_not_treated_as_paths() {
        use super::*;
        assert_eq!(AgentMode::requested_from([] as [&str; 0]), AgentMode::Off);
        assert_eq!(
            AgentMode::requested_from(["--explore-agent"]),
            AgentMode::Explore
        );
        assert_eq!(
            AgentMode::requested_from(["--enable-agent"]),
            AgentMode::Edit
        );
        assert_eq!(
            AgentMode::requested_from(["--enable-agent", "--explore-agent"]),
            AgentMode::Explore
        );
        assert_eq!(
            initial_path_from(["--explore-agent", "notes.txt"]),
            Some(PathBuf::from("notes.txt"))
        );
        assert_eq!(
            initial_path_from(["--enable-agent", "notes.txt"]),
            Some(PathBuf::from("notes.txt"))
        );
        assert_eq!(
            initial_path_from(["--enable-agent", "--explore-agent"]),
            None
        );
    }

    #[test]
    fn explore_reads_live_unsaved_text_without_advertising_propose() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.saved_text = "saved".into();
        app.text = "saved plus an unsaved thought".into();
        app.agent_mode = AgentMode::Explore;
        app.refresh_document();
        let snapshot = app.document.snapshot(
            &app.text,
            Some(&app.saved_text),
            app.is_dirty(),
            document::ExternalState::Unknown,
            false,
            true,
            app.agent_mode == AgentMode::Edit,
        );
        assert_eq!(snapshot.text, app.text);
        assert!(snapshot.dirty);
        assert_eq!(snapshot.capabilities, vec!["read"]);
        assert!(!snapshot.read_only);
    }

    #[test]
    fn explore_rejects_changes_before_a_proposal_is_registered() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.text = "original".into();
        app.saved_text = "original".into();
        app.refresh_document();
        let revision = app.document.identity().revision;
        let dirty = app.is_dirty();
        app.agent_mode = AgentMode::Explore;
        let error = app.require_agent_edit_access().unwrap_err();
        assert_eq!(error.code, "read_only");
        assert_eq!(app.text, "original");
        assert_eq!(app.document.identity().revision, revision);
        assert_eq!(app.is_dirty(), dirty);
        assert!(app.document.proposal_status("not-registered").is_none());
    }

    #[test]
    fn downgrading_rejects_pending_proposals_and_rechecks_application_access() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.text = "before".into();
        app.saved_text = "before".into();
        app.refresh_document();
        app.agent_mode = AgentMode::Edit;
        let patch = document::Patch {
            operation_id: "queued-change".into(),
            document_id: app.document.identity().document_id.clone(),
            base_revision: app.document.identity().revision,
            base_hash: document::hash(&app.text),
            edits: vec![document::Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after".into(),
            }],
        };
        app.document.propose(patch, &app.text).unwrap();
        app.pending_agent.push_back("queued-change".into());
        app.set_agent_mode(AgentMode::Explore);
        assert!(app.pending_agent.is_empty());
        assert_eq!(
            app.document.proposal_status("queued-change"),
            Some(document::ProposalStatus::Rejected)
        );
        assert!(matches!(
            app.approve_agent_proposal("queued-change"),
            Err(document::Error::ReadOnlyAccess)
        ));
        assert_eq!(app.text, "before");
        assert!(!app.is_dirty());
    }

    #[test]
    fn edit_applies_without_saving_can_be_undone_and_document_switch_invalidates_id() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "before").unwrap();
        let mut app = test_app(dir.path());
        app.path = Some(path.clone());
        app.text = "before".into();
        app.saved_text = "before".into();
        app.refresh_document();
        app.agent_mode = AgentMode::Edit;
        let old_document_id = app.document.identity().document_id.clone();
        let patch = document::Patch {
            operation_id: "edit-mode-change".into(),
            document_id: old_document_id.clone(),
            base_revision: app.document.identity().revision,
            base_hash: document::hash(&app.text),
            edits: vec![document::Edit {
                start_byte: 0,
                end_byte: 6,
                expected_text: "before".into(),
                replacement: "after".into(),
            }],
        };
        app.document.propose(patch, &app.text).unwrap();
        app.approve_agent_proposal("edit-mode-change").unwrap();
        app.refresh_document();
        assert_eq!(app.text, "after");
        assert!(app.is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");

        // Native/egui undo restores the previous buffer; observing it creates a
        // new revision and returns the normal dirty state to the saved baseline.
        app.text = "before".into();
        app.cache_valid = false;
        app.refresh_document();
        assert!(!app.is_dirty());
        assert!(app.document.identity().revision >= 2);

        app.replace_document();
        assert_ne!(app.document.identity().document_id, old_document_id);
        let stale_patch = document::Patch {
            operation_id: "old-document-change".into(),
            document_id: old_document_id,
            base_revision: 0,
            base_hash: document::hash(&app.text),
            edits: vec![document::Edit {
                start_byte: 0,
                end_byte: 0,
                expected_text: String::new(),
                replacement: "x".into(),
            }],
        };
        assert!(matches!(
            app.document.propose(stale_patch, &app.text),
            Err(document::Error::WrongDocument)
        ));
        assert_eq!(app.agent_mode, AgentMode::Edit);
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn native_newline_mode_uses_live_text_without_a_saved_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.text = "restored\r\ndraft".into();
        assert!(app.native_uses_crlf());

        app.path = Some(dir.path().join("note.txt"));
        app.saved_text = "saved\ntext".into();
        assert!(!app.native_uses_crlf());

        app.saved_text = "saved\r\ntext".into();
        app.text = "no newline".into();
        assert!(app.native_uses_crlf());
    }

    #[test]
    fn native_update_check_does_not_require_an_egui_frame() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        let ctx = app.ctx.clone();
        app.update = UpdateUi::Checking { user: true };
        app.poll_update_events(&ctx);
        assert!(matches!(app.update, UpdateUi::Checking { user: true }));
        app.update_tx
            .send((app.update_generation, UpdateEvent::UpToDate))
            .unwrap();
        app.poll_update_events(&ctx);
        assert!(matches!(app.update, UpdateUi::UpToDate));
        app.update = UpdateUi::Checking { user: true };
        app.update_tx
            .send((app.update_generation, UpdateEvent::Failed("offline".into())))
            .unwrap();
        app.poll_update_events(&ctx);
        assert!(matches!(&app.update, UpdateUi::Failed(message) if message == "offline"));
        app.update_tx
            .send((
                app.update_generation,
                UpdateEvent::Available {
                    version: "9.0.0".into(),
                    url: "https://example.invalid/update.zip".into(),
                },
            ))
            .unwrap();
        app.poll_update_events(&ctx);
        assert!(matches!(&app.update, UpdateUi::Available { version, .. } if version == "9.0.0"));
    }

    #[test]
    fn stale_dictionary_results_are_ignored() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.spell_generation = 2;
        app.spell = SpellState::Loading;
        app.spell_tx
            .send((1, SpellEvent::Failed(spell::SpellError::Unavailable)))
            .unwrap();
        app.poll_spell(&app.ctx.clone());
        assert!(matches!(app.spell, SpellState::Loading));
        assert!(app.error.is_none());
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        app.spell_tx.send((2, SpellEvent::Loaded(dict))).unwrap();
        app.poll_spell(&app.ctx.clone());
        assert!(matches!(app.spell, SpellState::Ready(_)));
    }

    #[test]
    fn committed_warning_updates_baseline_without_running_followup() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(&dir.path().join("recovery"));
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "new").unwrap();
        app.saved_text = "old".into();
        app.text = "new".into();
        app.after_save = Some(Action::New);
        app.file_tx
            .send(FileEvent::Save(
                path.clone(),
                "new".into(),
                Ok(storage::SaveOutcome {
                    durability_warning: Some(io::Error::other("fsync failed")),
                }),
            ))
            .unwrap();
        app.poll_files();
        assert_eq!(app.path.as_ref(), Some(&path));
        assert_eq!(app.saved_text, "new");
        assert_eq!(app.text, "new");
        assert!(!app.is_dirty());
        assert!(app.after_save.is_none());
        assert!(matches!(app.error, Some(AppError::Durability(_))));
    }

    #[test]
    fn failed_async_save_preserves_document_and_cancels_followup() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(&dir.path().join("recovery"));
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "external change").unwrap();
        app.path = Some(path.clone());
        app.saved_text = "original".into();
        app.text = "my edits".into();
        app.after_save = Some(Action::New);
        app.write_current();
        let event = app
            .file_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        app.file_tx.send(event).unwrap();
        app.poll_files();
        assert!(app.error.is_some());
        assert!(app.after_save.is_none());
        assert_eq!(app.text, "my edits");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external change");
    }

    #[test]
    fn async_save_finishes_before_followup() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(&dir.path().join("recovery"));
        let path = dir.path().join("note.txt");
        app.text = "my edits".into();
        app.after_save = Some(Action::New);
        app.begin_save(path.clone());
        assert_eq!(app.text, "my edits");
        let event = app
            .file_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        app.file_tx.send(event).unwrap();
        app.poll_files();
        assert!(app.text.is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "my edits");
    }

    #[test]
    fn cached_dirty_state_recognizes_undo() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.saved_text = "original".into();
        app.text = "edited".into();
        app.refresh_document();
        assert!(app.is_dirty());
        app.text = "original".into();
        app.cache_valid = false;
        app.refresh_document();
        assert!(!app.is_dirty());
        assert_eq!(app.counts, (8, 1));
    }

    #[test]
    fn opening_rtf_never_overwrites_sibling_text() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(&dir.path().join("recovery"));
        let path = dir.path().join("note.rtf");
        let txt = path.with_extension("txt");
        std::fs::write(&txt, "existing").unwrap();
        app.apply_open(
            path,
            Ok(large::Opened::Converted {
                text: "converted".into(),
                txt_path: txt.clone(),
            }),
        );
        app.refresh_document();
        assert!(app.is_dirty());
        assert!(app.path.is_none());
        assert_eq!(app.dialog_dir(), Some(dir.path()));
        assert_eq!(app.display_name_for_save(), "note.txt");
        assert_eq!(std::fs::read_to_string(txt).unwrap(), "existing");
        app.execute(Action::New);
        assert!(app.suggested_path.is_none());
    }

    #[test]
    #[ignore = "manual release performance measurement"]
    fn editor_performance() {
        use eframe::egui;
        for size in [200 * 1024, 1024 * 1024, 2 * 1024 * 1024] {
            for long_line in [false, true] {
                let unit = if long_line {
                    "abcdefghij "
                } else {
                    "abcdefghij\n"
                };
                let mut text = unit.repeat(size / unit.len());
                let ctx = egui::Context::default();
                let mut samples = Vec::new();
                let mut scrolling = Vec::new();
                for frame in 0..24 {
                    if frame > 0 && frame < 12 {
                        text.insert(text.len() / 2, 'x');
                    }
                    let start = std::time::Instant::now();
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(900.0, 600.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                egui::ScrollArea::vertical()
                                    .vertical_scroll_offset(if frame >= 12 {
                                        (frame - 12) as f32 * 100.0
                                    } else {
                                        0.0
                                    })
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(&mut text)
                                                .font(egui::TextStyle::Monospace)
                                                .desired_width(880.0),
                                        );
                                    });
                            });
                        },
                    );
                    if frame >= 12 {
                        scrolling.push(start.elapsed().as_secs_f64() * 1000.0);
                    } else if frame > 0 {
                        samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                samples.sort_by(f64::total_cmp);
                scrolling.sort_by(f64::total_cmp);
                eprintln!(
                    "{size} bytes long_line={long_line}: median {:.1} ms, max {:.1} ms",
                    samples[5], samples[10]
                );
                eprintln!(
                    "scroll median {:.1} ms, max {:.1} ms",
                    scrolling[6], scrolling[11]
                );
            }
        }
    }
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

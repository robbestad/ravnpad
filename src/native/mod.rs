//! Native window adapters share the existing document/storage/recovery controller.
//! No egui window or GPU renderer is created on macOS or Windows.
use super::*;
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    sync::mpsc,
};

unsafe extern "C" {
    fn rp_run();
    fn rp_smoke_test() -> i32;
    fn rp_document(text: *const c_char, length: usize, readonly: i32);
    fn rp_copy_text(length: *mut usize) -> *mut c_char;
    fn rp_free_text(text: *mut c_char);
    fn rp_state(
        title: *const c_char,
        path: *const c_char,
        status: *const c_char,
        dirty: i32,
        busy: i32,
        readonly: i32,
    );
    fn rp_preferences(font: *const c_char, points: f64, spell: i32, language: i32);
    fn rp_rebuild_menus();
    fn rp_close();
    fn rp_lock();
    fn rp_cancel_close();
}

enum Event {
    Action(i32),
    Changed,
    Open(PathBuf),
    Font(String, f32),
    View(f64),
}
struct Native {
    app: RavnPad,
    presented: u64,
    changed: bool,
    viewer_rx: mpsc::Receiver<(large::LargeView, Result<String, large::FileError>)>,
    viewer_tx: mpsc::Sender<(large::LargeView, Result<String, large::FileError>)>,
    viewer_busy: bool,
    viewer_text: String,
    offered_recovery: bool,
    binary_readonly: bool,
    pending_opens: std::collections::VecDeque<PathBuf>,
}
thread_local! {
    static APP: RefCell<Option<Native>> = const { RefCell::new(None) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static LABELS: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
}
fn c(text: &str) -> CString {
    CString::new(text.replace('\0', "�")).unwrap()
}
fn enqueue(event: Event) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}

pub fn run() {
    if std::env::args().any(|arg| arg == "--native-smoke-test") {
        let native_result = unsafe { rp_smoke_test() };
        // No menu command is requested by the smoke scenario. Inspect the real
        // bridge queue: text-control notifications must not become app actions.
        let unexpected_actions = EVENTS.with(|events| {
            events.borrow().iter().filter(|event| matches!(event, Event::Action(_))).count()
        });
        if unexpected_actions != 0 {
            eprintln!("Native notification routing: FAIL ({unexpected_actions} unexpected commands)");
        }
        std::process::exit(if unexpected_actions == 0 { native_result } else { 1 });
    }
    #[cfg(windows)]
    associate::register();
    update::cleanup_old();
    let app = RavnPad::new_core(
        egui::Context::default(),
        initial_path(),
        prefs::Prefs::load(),
        false,
    );
    labels(app.prefs.lang);
    let (viewer_tx, viewer_rx) = mpsc::channel();
    APP.with(|slot| {
        *slot.borrow_mut() = Some(Native {
            app,
            presented: u64::MAX,
            changed: false,
            viewer_rx,
            viewer_tx,
            viewer_busy: false,
            viewer_text: String::new(),
            offered_recovery: false,
            binary_readonly: false,
            pending_opens: std::collections::VecDeque::new(),
        })
    });
    unsafe {
        rp_run();
    }
    APP.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn rp_action(command: i32) {
    enqueue(Event::Action(command));
}
#[unsafe(no_mangle)]
pub extern "C" fn rp_changed() {
    enqueue(Event::Changed);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_open(path: *const c_char) {
    if !path.is_null() {
        enqueue(Event::Open(PathBuf::from(
            unsafe { CStr::from_ptr(path) }
                .to_string_lossy()
                .into_owned(),
        )));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_font(name: *const c_char, points: f64) {
    if !name.is_null() && points.is_finite() {
        enqueue(Event::Font(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
            (points as f32).clamp(6.0, 144.0),
        ));
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn rp_view(fraction: f64) {
    if fraction.is_finite() {
        enqueue(Event::View(fraction));
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn rp_label(id: i32) -> *const c_char {
    LABELS.with(|labels| {
        labels
            .borrow()
            .get(id.max(0) as usize)
            .map_or(c"".as_ptr(), |s| s.as_ptr())
    })
}
fn labels(lang: i18n::Lang) {
    let t = lang.text();
    let norwegian = matches!(lang, i18n::Lang::Bokmal | i18n::Lang::Nynorsk);
    let mut labels = vec![c(""); 115];
    for (id, value) in [
        (1, t.new),
        (2, t.open),
        (3, t.save),
        (4, t.save_as),
        (5, t.quit),
        (6, t.update_menu),
        (7, t.spellcheck),
        (8, lang.io_text().recovery),
        (9, t.font_label),
        (10, t.find),
        (11, t.replace),
        (12, "RavnPad"),
        (20, t.file_menu),
        (21, t.settings_menu),
        (22, t.language_menu),
        (23, t.help_menu),
        (24, if norwegian { "Rediger" } else { "Edit" }),
        (25, if norwegian { "Angre" } else { "Undo" }),
        (26, if norwegian { "Gjør om" } else { "Redo" }),
        (27, if norwegian { "Klipp ut" } else { "Cut" }),
        (28, if norwegian { "Kopier" } else { "Copy" }),
        (29, if norwegian { "Lim inn" } else { "Paste" }),
        (
            30,
            if norwegian {
                "Marker alt"
            } else {
                "Select All"
            },
        ),
        (31, if norwegian { "Neste" } else { "Next" }),
        (32, if norwegian { "Forrige" } else { "Previous" }),
        (33, t.replace_all),
        (34, t.cancel),
        (36, if norwegian { "Lukk" } else { "Close" }),
        (37, if norwegian { "Vindu" } else { "Window" }),
        (38, if norwegian { "Minimer" } else { "Minimize" }),
        (39, if norwegian { "Zoom" } else { "Zoom" }),
        (
            40,
            if norwegian {
                "Skjul RavnPad"
            } else {
                "Hide RavnPad"
            },
        ),
        (
            41,
            if norwegian {
                "Skjul andre"
            } else {
                "Hide Others"
            },
        ),
        (42, if norwegian { "Vis alle" } else { "Show All" }),
        (43, if norwegian { "Tjenester" } else { "Services" }),
        (
            35,
            if norwegian {
                "Åpne nylige"
            } else {
                "Open Recent"
            },
        ),
    ] {
        labels[id] = c(value);
    }
    for (i, lang) in i18n::Lang::ALL.iter().enumerate() {
        labels[100 + i] = c(lang.native_name());
    }
    LABELS.with(|slot| *slot.borrow_mut() = labels);
}

#[unsafe(no_mangle)]
pub extern "C" fn rp_tick() {
    // Native alerts spin a nested event loop; skip reentrant ticks.
    APP.with(|slot| {
        let Ok(mut guard) = slot.try_borrow_mut() else {
            return;
        };
        if let Some(native) = guard.as_mut() {
            native.tick();
        }
    });
}
impl Native {
    fn sync_text(&mut self) {
        if !self.changed || self.binary_readonly || self.app.large.is_some() || self.viewer_busy {
            return;
        }
        let mut len = 0;
        let text = unsafe { rp_copy_text(&mut len) };
        if text.is_null() {
            return;
        }
        let bytes = unsafe { std::slice::from_raw_parts(text.cast::<u8>(), len) };
        if let Ok(value) = std::str::from_utf8(bytes) {
            let normalized = value.replace("\r\n", "\n");
            let value = if self.app.saved_text.contains("\r\n") {
                normalized.replace('\n', "\r\n")
            } else {
                normalized
            };
            self.app.text = value;
            self.app.cache_valid = false;
            self.app.refresh_document();
        }
        unsafe {
            rp_free_text(text);
        }
        self.changed = false;
    }
    fn request(&mut self, action: Action) {
        if self.app.file_busy
            || self.viewer_busy
            || matches!(self.app.update, UpdateUi::Downloading)
        {
            return;
        }
        self.sync_text();
        // Native modal loops may dispatch input for an ownerless dialog. Keep the
        // document immutable while selecting paths or confirming unsaved changes.
        unsafe {
            rp_lock();
        }
        if matches!(action, Action::Quit) && !self.app.is_dirty() {
            self.app.close_requested = true;
            return;
        }
        self.app.request(action);
        if let Some(action) = self.app.confirm.take() {
            let t = self.app.t();
            match native_dialog::unsaved(
                t.unsaved_title,
                &t.unsaved(&self.app.display_name()),
                t.save,
                t.dont_save,
                t.cancel,
            ) {
                native_dialog::Confirm::Save => {
                    let started = if self.app.path.is_some() {
                        self.app.write_current()
                    } else {
                        self.app.start_save()
                    };
                    if started {
                        self.app.after_save = Some(action);
                    }
                }
                native_dialog::Confirm::Discard => {
                    self.app.clear_recovery();
                    if matches!(action, Action::Quit) {
                        self.app.close_requested = true;
                    } else {
                        self.app.execute(action);
                    }
                }
                native_dialog::Confirm::Cancel => {}
            }
        }
    }
    fn view(&mut self, fraction: Option<f64>) {
        if self.viewer_busy {
            return;
        }
        if let Some(mut view) = self.app.large.take() {
            self.viewer_busy = true;
            let tx = self.viewer_tx.clone();
            thread::spawn(move || {
                let result = view.native_window(fraction);
                let _ = tx.send((view, result));
            });
        }
    }
    fn tick(&mut self) {
        let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        for event in events {
            match event {
                Event::Changed => self.changed = true,
                Event::Open(path) => self.pending_opens.push_back(path),
                Event::View(fraction) => self.view(Some(fraction)),
                Event::Font(name, size) => {
                    self.app.prefs.font = name;
                    self.app.prefs.size = size;
                    self.app.save_prefs();
                }
                Event::Action(id) => match id {
                    1 => self.request(Action::New),
                    2 => self.request(Action::Open),
                    3 => self.request(Action::Save),
                    4 => self.request(Action::SaveAs),
                    5 => self.request(Action::Quit),
                    6 => self.request(Action::CheckUpdate),
                    7 => {
                        self.app.prefs.spellcheck = !self.app.prefs.spellcheck;
                        self.app.save_prefs();
                    }
                    8 => self.offered_recovery = false,
                    12 => {
                        let t = self.app.t();
                        info(
                            "RavnPad",
                            &format!("RavnPad {}\n© RavnPress", update::CURRENT),
                            t.ok,
                        );
                    }
                    100..115 => {
                        self.app.prefs.lang = i18n::Lang::ALL[(id - 100) as usize];
                        self.app.save_prefs();
                        labels(self.app.prefs.lang);
                        unsafe {
                            rp_rebuild_menus();
                        }
                    }
                    _ => {}
                },
            }
        }
        self.sync_text();
        if !self.app.file_busy
            && !self.viewer_busy
            && let Some(path) = self.pending_opens.pop_front()
        {
            self.request(Action::OpenPath(path));
        }
        self.app.poll_recovery();
        self.app.poll_files();
        self.app.refresh_document();
        if self.presented != self.app.document_generation && !self.app.file_busy {
            self.presented = self.app.document_generation;
            self.changed = false;
            self.binary_readonly = self.app.text.contains('\0');
            if self.app.large.is_some() {
                self.view(None);
            } else {
                let text = self.app.text.replace('\0', "␀");
                unsafe {
                    rp_document(
                        text.as_ptr().cast(),
                        text.len(),
                        self.binary_readonly as i32,
                    );
                }
            }
        }
        if let Ok((view, result)) = self.viewer_rx.try_recv() {
            self.viewer_busy = false;
            self.app.large = Some(view);
            match result {
                Ok(text) => {
                    self.viewer_text = text;
                    unsafe {
                        rp_document(self.viewer_text.as_ptr().cast(), self.viewer_text.len(), 1);
                    }
                }
                Err(error) => self.app.error = Some(AppError::File(error)),
            }
        }
        if !self.app.file_busy && !self.viewer_busy {
            if !self.offered_recovery && !self.app.recovery_candidates.is_empty() {
                self.offered_recovery = true;
                let candidates = self.app.recovery_candidates.clone();
                for candidate in candidates {
                    let t = self.app.prefs.lang.io_text();
                    unsafe {
                        rp_lock();
                    }
                    match native_dialog::unsaved(
                        t.recovery,
                        &candidate.preview,
                        t.restore,
                        t.delete,
                        self.app.t().cancel,
                    ) {
                        native_dialog::Confirm::Save => {
                            self.request(Action::Recover(candidate.path));
                            break;
                        }
                        native_dialog::Confirm::Discard => {
                            let _ = self
                                .app
                                .recovery
                                .tx
                                .send(recovery::Command::Delete(candidate.path.clone()));
                            self.app
                                .recovery_candidates
                                .retain(|c| c.path != candidate.path);
                        }
                        native_dialog::Confirm::Cancel => break,
                    }
                }
            }
            if let Some(error) = self.app.error.take() {
                native_dialog::error(self.app.t().error_title, &self.app.error_message(error));
            }
        }
        self.app.poll_update(&self.app.ctx.clone());
        let t = self.app.t();
        match std::mem::replace(&mut self.app.update, UpdateUi::Idle) {
            UpdateUi::Available { version, url } => {
                if matches!(
                    native_dialog::unsaved(
                        t.update_available_title,
                        &t.update_available(&version, update::CURRENT),
                        t.update_now,
                        t.update_later,
                        t.cancel
                    ),
                    native_dialog::Confirm::Save
                ) {
                    self.request(Action::InstallUpdate { url });
                }
            }
            UpdateUi::UpToDate => info(t.help_menu, &t.update_uptodate_msg(update::CURRENT), t.ok),
            UpdateUi::Failed(message) => native_dialog::error(t.update_failed, &message),
            UpdateUi::Unsupported => info(t.help_menu, t.update_unsupported, t.ok),
            other => self.app.update = other,
        }
        let busy = self.app.file_busy
            || self.viewer_busy
            || matches!(self.app.update, UpdateUi::Downloading);
        let status = if busy {
            self.app.prefs.lang.io_text().busy.to_owned()
        } else if let Some(view) = &self.app.large {
            view.status(t.view_readonly, t.decimal)
        } else if self.binary_readonly {
            t.view_readonly.to_owned()
        } else {
            format!(
                "{} · {}",
                t.chars(self.app.counts.0),
                t.words(self.app.counts.1)
            )
        };
        #[cfg(target_os = "macos")]
        let title = c(&self.app.display_name());
        #[cfg(windows)]
        let title = c(&self.app.title());
        let path = c(&self
            .app
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default());
        let font = c(&self.app.prefs.font);
        unsafe {
            rp_state(
                title.as_ptr(),
                path.as_ptr(),
                c(&status).as_ptr(),
                self.app.is_dirty() as i32,
                busy as i32,
                (self.binary_readonly || self.app.large.is_some() || self.viewer_busy) as i32,
            );
            rp_preferences(
                font.as_ptr(),
                self.app.prefs.size as f64,
                self.app.prefs.spellcheck as i32,
                i18n::Lang::ALL
                    .iter()
                    .position(|l| *l == self.app.prefs.lang)
                    .unwrap_or(0) as i32,
            );
            if self.app.close_requested {
                self.app.recovery.finish();
                rp_close();
            } else if !self.app.file_busy {
                rp_cancel_close();
            }
        }
    }
}
fn info(title: &str, message: &str, ok: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::OkCustom(ok.to_owned()))
        .show();
}

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
    fn rp_replace_text(text: *const c_char, length: usize);
    fn rp_copy_text(length: *mut usize) -> *mut c_char;
    fn rp_free_text(text: *mut c_char);
    fn rp_state(
        title: *const c_char,
        path: *const c_char,
        status: *const c_char,
        dirty: i32,
        busy: i32,
        readonly: i32,
        large: i32,
    );
    fn rp_find_result(length: usize);
    fn rp_preferences(font: *const c_char, points: f64, spell: i32, language: i32);
    fn rp_wrap(enabled: i32);
    fn rp_theme(preference: i32);
    fn rp_read_position() -> f64;
    fn rp_restore_position(fraction: f64);
    fn rp_rebuild_menus();
    fn rp_close();
    fn rp_lock();
    fn rp_cancel_close();
    fn rp_confirm(
        title: *const c_char,
        body: *const c_char,
        accept: *const c_char,
        discard: *const c_char,
        cancel: *const c_char,
    ) -> i32;
}

enum Event {
    Action(i32),
    Changed,
    Open(PathBuf),
    Font(String, f32),
    View(f64),
    FindLarge(String, bool, bool, bool),
}
enum SearchOutcome {
    Found {
        text: String,
        position: u64,
        length: usize,
    },
    NotFound,
    Failed(large::FileError),
}
struct Native {
    app: RavnPad,
    presented: u64,
    changed: bool,
    viewer_rx: mpsc::Receiver<(large::LargeView, Result<String, large::FileError>)>,
    viewer_tx: mpsc::Sender<(large::LargeView, Result<String, large::FileError>)>,
    search_rx: mpsc::Receiver<(large::LargeView, String, SearchOutcome)>,
    search_tx: mpsc::Sender<(large::LargeView, String, SearchOutcome)>,
    viewer_busy: bool,
    viewer_text: String,
    offered_recovery: bool,
    binary_readonly: bool,
    pending_opens: std::collections::VecDeque<PathBuf>,
    last_find: Option<(String, u64)>,
    large_document: bool,
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
fn confirm(
    title: &str,
    body: &str,
    accept: &str,
    discard: &str,
    cancel: &str,
) -> native_dialog::Confirm {
    let title = c(title);
    let body = c(body);
    let accept = c(accept);
    let discard = c(discard);
    let cancel = c(cancel);
    match unsafe {
        rp_confirm(
            title.as_ptr(),
            body.as_ptr(),
            accept.as_ptr(),
            discard.as_ptr(),
            cancel.as_ptr(),
        )
    } {
        1 => native_dialog::Confirm::Save,
        2 => native_dialog::Confirm::Discard,
        _ => native_dialog::Confirm::Cancel,
    }
}

pub fn run() {
    if std::env::args().any(|arg| arg == "--native-smoke-test") {
        let native_result = unsafe { rp_smoke_test() };
        // No menu command is requested by the smoke scenario. Inspect the real
        // bridge queue: text-control notifications must not become app actions.
        let unexpected_actions = EVENTS.with(|events| {
            events
                .borrow()
                .iter()
                .filter(|event| matches!(event, Event::Action(_)))
                .count()
        });
        if unexpected_actions != 0 {
            eprintln!(
                "Native notification routing: FAIL ({unexpected_actions} unexpected commands)"
            );
        }
        std::process::exit(if unexpected_actions == 0 {
            native_result
        } else {
            1
        });
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
    let (search_tx, search_rx) = mpsc::channel();
    APP.with(|slot| {
        *slot.borrow_mut() = Some(Native {
            app,
            presented: u64::MAX,
            changed: false,
            viewer_rx,
            viewer_tx,
            search_rx,
            search_tx,
            viewer_busy: false,
            viewer_text: String::new(),
            offered_recovery: false,
            binary_readonly: false,
            pending_opens: std::collections::VecDeque::new(),
            last_find: None,
            large_document: false,
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
pub unsafe extern "C" fn rp_find_large(
    query: *const c_char,
    backwards: i32,
    match_case: i32,
    whole_word: i32,
) -> i32 {
    if query.is_null() {
        return 0;
    }
    let active = APP.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|native| native.large_document))
            .unwrap_or(false)
    });
    if active {
        enqueue(Event::FindLarge(
            unsafe { CStr::from_ptr(query) }
                .to_string_lossy()
                .into_owned(),
            backwards != 0,
            match_case != 0,
            whole_word != 0,
        ));
    }
    active as i32
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
    let mut labels = vec![c(""); 210];
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
        (33, t.replace_all),
        (34, t.cancel),
        (44, t.new_window),
        (45, t.line_wrap),
        (46, t.theme),
        (47, t.theme_system),
        (48, t.theme_light),
        (49, t.theme_dark),
        (50, t.no_matches),
        (51, t.whole_word),
    ] {
        labels[id] = c(value);
    }
    for id in (24..=32).chain(35..=43) {
        labels[id] = c(lang.native_label(id as i32).expect("native label"));
    }
    for (i, lang) in i18n::Lang::ALL.iter().enumerate() {
        labels[100 + i] = c(lang.native_name());
    }
    let recent = prefs::Prefs::load().recent;
    for (i, label) in prefs::recent_labels(&recent).iter().take(10).enumerate() {
        labels[200 + i] = c(label);
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
        if self.app.large.is_none() {
            self.app.editor_scroll_y = unsafe { rp_read_position() } as f32;
        }
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
            match confirm(
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
                Event::FindLarge(query, backwards, match_case, whole_word) => {
                    if !self.viewer_busy
                        && let Some(mut view) = self.app.large.take()
                    {
                        let same = self
                            .last_find
                            .as_ref()
                            .is_some_and(|(previous, _)| previous == &query);
                        let current = self
                            .last_find
                            .as_ref()
                            .map(|(_, position)| *position)
                            .unwrap_or_else(|| view.offset());
                        let from = if same {
                            if backwards {
                                current
                            } else {
                                current.saturating_add(1)
                            }
                        } else {
                            current
                        };
                        self.viewer_busy = true;
                        let tx = self.search_tx.clone();
                        thread::spawn(move || {
                            let outcome = match view
                                .find_from(&query, from, backwards, match_case, whole_word)
                            {
                                Ok(Some((position, byte_length))) => {
                                    view.set_offset(position);
                                    match view.native_window(None) {
                                        Ok(text) => {
                                            let length = text
                                                .get(..byte_length)
                                                .map(|matched| matched.encode_utf16().count())
                                                .unwrap_or_else(|| query.encode_utf16().count());
                                            SearchOutcome::Found {
                                                text,
                                                position,
                                                length,
                                            }
                                        }
                                        Err(error) => SearchOutcome::Failed(error),
                                    }
                                }
                                Ok(None) => SearchOutcome::NotFound,
                                Err(error) => SearchOutcome::Failed(error),
                            };
                            let _ = tx.send((view, query, outcome));
                        });
                    }
                }
                Event::Font(name, size) => {
                    self.app.prefs.font = name;
                    self.app.prefs.size = size;
                    self.app.save_prefs();
                }
                Event::Action(id) => match id {
                    1 => self.request(Action::New),
                    44 => self.request(Action::NewWindow),
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
                    45 => {
                        self.app.prefs.line_wrap = !self.app.prefs.line_wrap;
                        self.app.save_prefs();
                    }
                    47..=49 => {
                        self.app.prefs.theme = match id {
                            48 => prefs::ThemePref::Light,
                            49 => prefs::ThemePref::Dark,
                            _ => prefs::ThemePref::System,
                        };
                        self.app.save_prefs();
                    }
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
                    200..210 => {
                        if let Some(path) = self.app.prefs.recent.get((id - 200) as usize).cloned()
                        {
                            self.request(Action::OpenPath(path));
                        }
                    }
                    _ => {}
                },
            }
        }
        self.sync_text();
        self.app.poll_agent();
        if let Some(operation_id) = self.app.pending_agent.pop_front() {
            if let Some(proposal) = self.app.document.proposal(&operation_id).cloned() {
                let preview = proposal_preview(&proposal.before, &proposal.after, &proposal.hunks);
                unsafe {
                    rp_lock();
                }
                match confirm(
                    "Agent suggestion",
                    &preview,
                    "Apply",
                    "Reject",
                    self.app.t().cancel,
                ) {
                    native_dialog::Confirm::Save => {
                        match self.app.approve_agent_proposal(&operation_id) {
                            Ok(text) => unsafe {
                                rp_replace_text(text.as_ptr().cast(), text.len());
                            },
                            Err(error) => {
                                self.app.reject_agent_proposal(&operation_id);
                                self.app.error = Some(AppError::Agent(error));
                            }
                        }
                    }
                    native_dialog::Confirm::Discard => {
                        self.app.reject_agent_proposal(&operation_id)
                    }
                    native_dialog::Confirm::Cancel => {
                        self.app.pending_agent.push_back(operation_id)
                    }
                }
            }
        }
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
            labels(self.app.prefs.lang);
            unsafe {
                rp_rebuild_menus();
            }
            self.binary_readonly = self.app.text.contains('\0');
            self.last_find = None;
            self.large_document = self.app.large.is_some();
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
                    rp_restore_position(self.app.editor_scroll_y as f64);
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
        if let Ok((view, query, outcome)) = self.search_rx.try_recv() {
            self.viewer_busy = false;
            self.app.large = Some(view);
            match outcome {
                SearchOutcome::Found {
                    text,
                    position,
                    length,
                } => {
                    self.last_find = Some((query, position));
                    self.viewer_text = text;
                    unsafe {
                        rp_document(self.viewer_text.as_ptr().cast(), self.viewer_text.len(), 1);
                        rp_find_result(length);
                    }
                }
                SearchOutcome::NotFound => unsafe { rp_find_result(0) },
                SearchOutcome::Failed(error) => self.app.error = Some(AppError::File(error)),
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
                    match confirm(
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
        self.app.poll_update_events(&self.app.ctx.clone());
        let t = self.app.t();
        match std::mem::replace(&mut self.app.update, UpdateUi::Idle) {
            UpdateUi::Available { version, url } => {
                if matches!(
                    confirm(
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
                self.large_document as i32,
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
            rp_wrap(self.app.prefs.line_wrap as i32);
            rp_theme(match self.app.prefs.theme {
                prefs::ThemePref::System => 0,
                prefs::ThemePref::Light => 1,
                prefs::ThemePref::Dark => 2,
            });
            if self.app.close_requested {
                self.app.recovery.finish();
                rp_close();
            } else if !self.app.file_busy {
                rp_cancel_close();
            }
        }
    }
}
fn proposal_preview(before: &str, after: &str, hunks: &[document::ProposalHunk]) -> String {
    fn boundary_at_or_before(value: &str, mut index: usize) -> usize {
        index = index.min(value.len());
        while !value.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn excerpt(value: &str, changed_start: usize, changed_end: usize) -> String {
        const CONTEXT: usize = 256;
        const LIMIT: usize = 1_400;
        let start = boundary_at_or_before(value, changed_start.saturating_sub(CONTEXT));
        let end = boundary_at_or_before(value, changed_end.saturating_add(CONTEXT));
        let end = end.max(changed_end).min(value.len());
        let leading = if start > 0 { "…\n" } else { "" };
        let trailing = if end < value.len() { "\n…" } else { "" };
        let available = LIMIT.saturating_sub(leading.len() + trailing.len());
        if end - start <= available {
            return format!("{leading}{}{trailing}", &value[start..end]);
        }

        let half = available.saturating_sub("\n… omitted …\n".len()) / 2;
        let head_end = boundary_at_or_before(value, start.saturating_add(half));
        let tail_start = boundary_at_or_before(value, end.saturating_sub(half));
        format!(
            "{leading}{}\n… omitted …\n{}{trailing}",
            &value[start..head_end],
            &value[tail_start..end]
        )
    }

    hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| {
            format!(
                "Change {}/{}\n\nBefore:\n{}\n\nAfter:\n{}",
                index + 1,
                hunks.len(),
                excerpt(before, hunk.before_start, hunk.before_end),
                excerpt(after, hunk.after_start, hunk.after_end)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n────────────────────\n\n")
}
fn info(title: &str, message: &str, ok: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::OkCustom(ok.to_owned()))
        .show();
}

#[cfg(test)]
mod tests {
    use super::proposal_preview;
    use crate::document::ProposalHunk;

    #[test]
    fn proposal_preview_centers_a_late_change() {
        let before = format!("{}before{}", "a".repeat(8_000), "z".repeat(8_000));
        let after = format!("{}after{}", "a".repeat(8_000), "z".repeat(8_000));
        let preview = proposal_preview(
            &before,
            &after,
            &[ProposalHunk {
                before_start: 8_000,
                before_end: 8_006,
                after_start: 8_000,
                after_end: 8_005,
            }],
        );
        assert!(preview.contains("before"));
        assert!(preview.contains("after"));
        assert!(!preview.starts_with(&"a".repeat(4_000)));
        assert!(preview.len() < 3_000);
    }

    #[test]
    fn proposal_preview_keeps_utf8_boundaries() {
        let prefix = "😀".repeat(2_000);
        let before = format!("{prefix}før");
        let after = format!("{prefix}etter");
        let preview = proposal_preview(
            &before,
            &after,
            &[ProposalHunk {
                before_start: prefix.len(),
                before_end: before.len(),
                after_start: prefix.len(),
                after_end: after.len(),
            }],
        );
        assert!(preview.contains("før"));
        assert!(preview.contains("etter"));
    }

    #[test]
    fn proposal_preview_shows_every_separated_hunk() {
        let gap = "x".repeat(5_000);
        let before = format!("old-one{gap}old-two{gap}old-three");
        let after = format!("new-one{gap}new-two{gap}new-three");
        let hunks = ["one", "two", "three"].map(|suffix| {
            let old = format!("old-{suffix}");
            let new = format!("new-{suffix}");
            let before_start = before.find(&old).unwrap();
            let after_start = after.find(&new).unwrap();
            ProposalHunk {
                before_start,
                before_end: before_start + old.len(),
                after_start,
                after_end: after_start + new.len(),
            }
        });
        let preview = proposal_preview(&before, &after, &hunks);
        for marker in [
            "old-one",
            "new-one",
            "old-two",
            "new-two",
            "old-three",
            "new-three",
        ] {
            assert!(preview.contains(marker), "missing {marker}");
        }
        assert!(preview.contains("Change 1/3"));
        assert!(preview.contains("Change 2/3"));
        assert!(preview.contains("Change 3/3"));
    }
}

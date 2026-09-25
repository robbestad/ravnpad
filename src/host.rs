//! Document ownership and commands shared by the windowed and headless hosts.

use crate::{agent, document, prefs, recovery, storage};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "macos", windows))]
const EDIT_LIMIT: usize = 16 * 1024 * 1024;
#[cfg(not(any(target_os = "macos", windows)))]
const EDIT_LIMIT: usize = 2 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 256;
const GUI_SESSION_TIMEOUT: Duration = Duration::from_secs(10);

fn bound_history(history: &mut Vec<String>) {
    while history.len() > MAX_HISTORY_ENTRIES
        || (history.len() > 1 && history.iter().map(String::len).sum::<usize>() > MAX_HISTORY_BYTES)
    {
        history.remove(0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMode {
    Off,
    Explore,
    Edit,
}

impl AgentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "explore" => Some(Self::Explore),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Explore => "explore",
            Self::Edit => "edit",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostState {
    pub identity: document::Identity,
    pub text: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub agent_mode: String,
    pub read_only: bool,
}

pub struct Core {
    pub document: document::Document,
    text: String,
    saved_text: String,
    path: Option<PathBuf>,
    read_only: bool,
    undo: Vec<String>,
    redo: Vec<String>,
    mode: AgentMode,
    recovery: recovery::Recovery,
    recovery_due: Option<Instant>,
    recovered_from: Option<PathBuf>,
    gui_session: Option<String>,
    gui_last_seen: Option<Instant>,
    stop: bool,
}

impl Core {
    pub fn new(path: Option<PathBuf>, mode: AgentMode) -> io::Result<Self> {
        let (text, read_only) = match &path {
            Some(path) if std::fs::metadata(path)?.len() > EDIT_LIMIT as u64 => {
                (String::new(), true)
            }
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => (text, false),
                Err(error) if error.kind() == io::ErrorKind::InvalidData => (String::new(), true),
                Err(error) => return Err(error),
            },
            None => (String::new(), false),
        };
        Ok(Self {
            document: document::Document::new(&text, EDIT_LIMIT),
            saved_text: text.clone(),
            text,
            path,
            read_only,
            undo: Vec::new(),
            redo: Vec::new(),
            mode,
            recovery: recovery::Recovery::start(),
            recovery_due: None,
            recovered_from: None,
            gui_session: None,
            gui_last_seen: None,
            stop: false,
        })
    }

    pub fn state(&self) -> HostState {
        HostState {
            identity: self.document.identity().clone(),
            text: self.text.clone(),
            path: self.path.clone(),
            dirty: self.dirty(),
            undo_depth: self.undo.len(),
            redo_depth: self.redo.len(),
            agent_mode: self.mode.name().into(),
            read_only: self.read_only,
        }
    }

    pub fn dirty(&self) -> bool {
        self.text != self.saved_text
    }

    fn changed(&mut self, text: String) {
        if self.text == text {
            return;
        }
        self.undo.push(std::mem::replace(&mut self.text, text));
        bound_history(&mut self.undo);
        self.redo.clear();
        self.document.observe_text(&self.text);
        self.recovery_due = Some(Instant::now() + Duration::from_secs(2));
    }

    fn snapshot(&self) {
        let value = self.dirty().then(|| self.text.clone());
        let _ = self.recovery.tx.send(recovery::Command::Snapshot(value));
    }

    fn flush_recovery_if_due(&mut self) {
        if self.recovery_due.is_some_and(|due| Instant::now() >= due) {
            self.recovery_due = None;
            self.snapshot();
        }
    }

    fn save(&mut self) -> io::Result<storage::SaveOutcome> {
        if self.read_only {
            return Err(io::Error::other("this document is read-only"));
        }
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "document has no path; use Save As",
            )
        })?;
        let outcome =
            storage::save_checked(path, self.text.as_bytes(), Some(self.saved_text.as_bytes()))?;
        self.saved_text.clone_from(&self.text);
        self.recovery_due = None;
        self.snapshot();
        if let Some(path) = self.recovered_from.take() {
            let _ = self.recovery.tx.send(recovery::Command::Delete(path));
        }
        Ok(outcome)
    }

    fn save_as(&mut self, path: PathBuf) -> io::Result<storage::SaveOutcome> {
        if self.read_only {
            return Err(io::Error::other("this document is read-only"));
        }
        let expected = if self.path.as_ref() == Some(&path) {
            Some(self.saved_text.as_bytes().to_vec())
        } else {
            match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            }
        };
        let outcome = storage::save_checked(&path, self.text.as_bytes(), expected.as_deref())?;
        self.path = Some(path);
        self.saved_text.clone_from(&self.text);
        self.recovery_due = None;
        self.snapshot();
        if let Some(path) = self.recovered_from.take() {
            let _ = self.recovery.tx.send(recovery::Command::Delete(path));
        }
        Ok(outcome)
    }

    fn recover(&mut self, path: PathBuf) -> io::Result<()> {
        let text = std::fs::read_to_string(&path)?;
        if text.len() > EDIT_LIMIT {
            return Err(io::Error::other("recovery copy exceeds edit limit"));
        }
        self.text = text;
        self.saved_text.clear();
        self.path = None;
        self.read_only = false;
        self.undo.clear();
        self.redo.clear();
        self.document.replace_document(&self.text);
        self.recovered_from = Some(path);
        self.recovery_due = None;
        self.snapshot();
        Ok(())
    }

    fn error(code: &'static str, message: impl Into<String>) -> agent::Response {
        agent::Response::Error {
            error: agent::ApiError::new(code, message),
        }
    }

    fn gui_authorized(&mut self, token: &str, session: &str, owner: &str) -> bool {
        if token != owner || self.gui_session.as_deref() != Some(session) {
            return false;
        }
        self.gui_last_seen = Some(Instant::now());
        true
    }

    fn gui_state(&self, session: Option<String>) -> agent::Response {
        agent::Response::Gui {
            state: self.state(),
            session,
        }
    }

    fn undo_redo(
        &mut self,
        token: &str,
        owner: &str,
        session: &str,
        base_revision: u64,
        undo: bool,
    ) -> agent::Response {
        if !self.gui_authorized(token, session, owner) {
            return Self::error("unauthorized", "GUI session required");
        }
        if base_revision != self.document.identity().revision {
            return Self::error("stale_revision", "document changed; reread before editing");
        }
        let source = if undo { &mut self.undo } else { &mut self.redo };
        if let Some(text) = source.pop() {
            let target = if undo { &mut self.redo } else { &mut self.undo };
            target.push(std::mem::replace(&mut self.text, text));
            bound_history(target);
            self.document.observe_text(&self.text);
            self.recovery_due = Some(Instant::now() + Duration::from_secs(2));
        }
        self.gui_state(None)
    }

    pub fn handle(
        &mut self,
        request: agent::Request,
        owner: &str,
        agent_token: &str,
    ) -> agent::Response {
        use agent::Request as R;
        let token = request.token().to_owned();
        match request {
            R::DocumentStatus { .. } => {
                if token != agent_token || self.mode == AgentMode::Off {
                    return Self::error("unauthorized", "agent access unavailable");
                }
                agent::Response::Document {
                    document: self.document.identity().clone(),
                }
            }
            R::DocumentRead {
                document_id,
                offset,
                limit,
                ..
            } => {
                if token != agent_token || self.mode == AgentMode::Off {
                    return Self::error("unauthorized", "agent access unavailable");
                }
                if document_id != self.document.identity().document_id {
                    return Self::error("wrong_document", "document is no longer open");
                }
                if self.read_only {
                    return Self::error(
                        "content_unavailable",
                        "large or binary document content is not exposed by this API",
                    );
                }
                let start = offset.unwrap_or(0);
                let end = limit
                    .map(|n| start.saturating_add(n).min(self.text.len()))
                    .unwrap_or(self.text.len());
                if start > self.text.len()
                    || !self.text.is_char_boundary(start)
                    || !self.text.is_char_boundary(end)
                {
                    return Self::error("invalid_range", "read range is not on UTF-8 boundaries");
                }
                if limit.is_none() && self.text.len() > 512 * 1024 {
                    return Self::error(
                        "range_required",
                        "documents over 512 KiB require offset and limit",
                    );
                }
                let complete = start == 0 && end == self.text.len();
                let snapshot = if complete {
                    self.document.snapshot(
                        &self.text,
                        self.path.as_ref().map(|_| self.saved_text.as_str()),
                        self.dirty(),
                        document::ExternalState::Unknown,
                        false,
                        true,
                        self.mode == AgentMode::Edit,
                    )
                } else {
                    self.document.ranged_snapshot(
                        &self.text[start..end],
                        self.dirty(),
                        document::ExternalState::Unknown,
                        false,
                    )
                };
                agent::bounded_snapshot_response(snapshot)
            }
            R::DocumentPropose { patch, .. } => {
                if token != agent_token || self.mode != AgentMode::Edit {
                    return Self::error("read_only", "agent edit access is disabled");
                }
                if self.read_only {
                    return Self::error("read_only", "this document cannot be edited");
                }
                let proposal = match self.document.propose(patch, &self.text) {
                    Ok(p) => p.clone(),
                    Err(e) => {
                        return agent::Response::Error {
                            error: agent::ApiError::document(e),
                        };
                    }
                };
                match self.document.proposal_status(&proposal.operation_id) {
                    Some(document::ProposalStatus::Applied) => {
                        return agent::Response::Applied {
                            proposal: (&proposal).into(),
                        };
                    }
                    Some(document::ProposalStatus::Rejected) => {
                        return agent::Response::Rejected {
                            proposal: (&proposal).into(),
                        };
                    }
                    _ => {}
                }
                let text = match self.document.approve(&proposal.operation_id, &self.text) {
                    Ok(t) => t,
                    Err(e) => {
                        return agent::Response::Error {
                            error: agent::ApiError::document(e),
                        };
                    }
                };
                self.changed(text);
                if let Err(e) = self
                    .document
                    .finish_approval(&proposal.operation_id, &self.text)
                {
                    return agent::Response::Error {
                        error: agent::ApiError::document(e),
                    };
                }
                agent::Response::Applied {
                    proposal: (&proposal).into(),
                }
            }
            R::DocumentSave { document_id, .. } => {
                if token != agent_token || self.mode != AgentMode::Edit {
                    return Self::error("read_only", "agent save access is disabled");
                }
                if document_id != self.document.identity().document_id {
                    return Self::error("wrong_document", "document is no longer open");
                }
                match self.save() {
                    Ok(result) => agent::Response::Saved {
                        revision: self.document.identity().revision,
                        durability_warning: result.durability_warning.map(|e| e.to_string()),
                    },
                    Err(e) => Self::error(
                        if e.kind() == io::ErrorKind::AlreadyExists {
                            "file_conflict"
                        } else {
                            "save_failed"
                        },
                        e.to_string(),
                    ),
                }
            }
            R::HostStop { discard, .. } => {
                if token != owner {
                    return Self::error("unauthorized", "owner token required");
                }
                if self.dirty() && !discard {
                    return Self::error("unsaved_changes", "save or use --discard to stop");
                }
                if discard && self.dirty() {
                    self.recovery_due = None;
                    self.snapshot();
                } else {
                    let _ = self.recovery.tx.send(recovery::Command::Snapshot(None));
                }
                self.stop = true;
                agent::Response::Stopped
            }
            R::HostSetMode { mode, .. } => {
                if token != owner {
                    return Self::error("unauthorized", "owner token required");
                }
                let Some(mode) = AgentMode::parse(&mode) else {
                    return Self::error("invalid_mode", "expected off, explore or edit");
                };
                self.mode = mode;
                if mode != AgentMode::Edit {
                    self.document.reject_pending();
                }
                agent::Response::Mode {
                    mode: mode.name().into(),
                }
            }
            R::GuiAttach { .. } => {
                if token != owner {
                    return Self::error("unauthorized", "owner token required");
                }
                if self
                    .gui_last_seen
                    .is_some_and(|seen| seen.elapsed() >= GUI_SESSION_TIMEOUT)
                {
                    self.gui_session = None;
                    self.gui_last_seen = None;
                }
                if self.gui_session.is_some() {
                    return Self::error("gui_busy", "an editing GUI is already attached");
                }
                let session = match agent::new_token() {
                    Ok(s) => s,
                    Err(e) => return Self::error("token_failed", e.to_string()),
                };
                self.gui_session = Some(session.clone());
                self.gui_last_seen = Some(Instant::now());
                self.gui_state(Some(session))
            }
            R::GuiDetach { session, .. } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                self.gui_session = None;
                self.gui_last_seen = None;
                self.recovery_due = None;
                self.snapshot();
                self.gui_state(None)
            }
            R::GuiState { session, .. } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                self.gui_state(None)
            }
            R::GuiEdit {
                session,
                base_revision,
                text,
                ..
            } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                if self.read_only {
                    return Self::error("read_only", "this document cannot be edited");
                }
                if base_revision != self.document.identity().revision {
                    return Self::error(
                        "stale_revision",
                        "document changed; reread before editing",
                    );
                }
                if text.len() > EDIT_LIMIT {
                    return Self::error("result_too_large", "document exceeds edit limit");
                }
                self.changed(text);
                self.gui_state(None)
            }
            R::GuiUndo {
                session,
                base_revision,
                ..
            } => self.undo_redo(&token, owner, &session, base_revision, true),
            R::GuiRedo {
                session,
                base_revision,
                ..
            } => self.undo_redo(&token, owner, &session, base_revision, false),
            R::GuiSave {
                session,
                base_revision,
                ..
            } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                if base_revision != self.document.identity().revision {
                    return Self::error("stale_revision", "document changed; reread before saving");
                }
                match self.save() {
                    Ok(result) => agent::Response::Saved {
                        revision: self.document.identity().revision,
                        durability_warning: result.durability_warning.map(|e| e.to_string()),
                    },
                    Err(e) => Self::error(
                        if e.kind() == io::ErrorKind::AlreadyExists {
                            "file_conflict"
                        } else {
                            "save_failed"
                        },
                        e.to_string(),
                    ),
                }
            }
            R::GuiSaveAs {
                session,
                base_revision,
                path,
                ..
            } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                if base_revision != self.document.identity().revision {
                    return Self::error("stale_revision", "document changed; reread before saving");
                }
                match self.save_as(path) {
                    Ok(result) => agent::Response::Saved {
                        revision: self.document.identity().revision,
                        durability_warning: result.durability_warning.map(|e| e.to_string()),
                    },
                    Err(e) => Self::error(
                        if e.kind() == io::ErrorKind::AlreadyExists {
                            "file_conflict"
                        } else {
                            "save_failed"
                        },
                        e.to_string(),
                    ),
                }
            }
            R::GuiRecover { session, path, .. } => {
                if !self.gui_authorized(&token, &session, owner) {
                    return Self::error("unauthorized", "GUI session required");
                }
                match self.recover(path) {
                    Ok(()) => self.gui_state(None),
                    Err(error) => Self::error("recovery_failed", error.to_string()),
                }
            }
        }
    }
}

pub fn run(path: Option<PathBuf>, mode: AgentMode) -> io::Result<()> {
    let mut core = Core::new(path, mode)?;
    let instance = core.document.identity().instance_id.clone();
    let base = prefs::config_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "configuration directory unavailable",
        )
    })?;
    cleanup_stale_endpoints(&base);
    let server = agent::Server::start_in(&instance, base.join("host"), Arc::new(|| {}))?;
    let mut agent_token = agent::new_token()?;
    let agent_dir = base.join("agent");
    let mut agent_path = None;
    if mode != AgentMode::Off {
        agent_path = Some(agent::publish_endpoint(
            &agent_dir,
            &instance,
            server.address(),
            &agent_token,
        )?);
    }
    println!(
        "{}",
        serde_json::json!({"instance_id":instance,"document_id":core.document.identity().document_id})
    );
    use io::Write as _;
    io::stdout().flush()?;
    while !core.stop {
        let timeout = core
            .recovery_due
            .map(|due| due.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        let pending = match server.recv_timeout(timeout) {
            Ok(request) => Some(request),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("host IPC listener stopped"));
            }
        };
        if let Some(request) = pending {
            let response = core.handle(request.request.clone(), server.token(), &agent_token);
            request.respond(response);
            if core.mode == AgentMode::Off && agent_path.is_some() {
                if let Some(path) = agent_path.take() {
                    let _ = std::fs::remove_file(path);
                }
            }
            if core.mode != AgentMode::Off && agent_path.is_none() {
                agent_token = agent::new_token()?;
                agent_path = Some(agent::publish_endpoint(
                    &agent_dir,
                    &instance,
                    server.address(),
                    &agent_token,
                )?);
            }
        }
        core.flush_recovery_if_due();
    }
    if let Some(path) = agent_path {
        let _ = std::fs::remove_file(path);
    }
    core.recovery.finish();
    Ok(())
}

fn cleanup_stale_endpoints(base: &std::path::Path) {
    for name in ["host", "agent"] {
        let Ok(entries) = std::fs::read_dir(base.join(name)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(endpoint) = serde_json::from_slice::<agent::Endpoint>(&bytes) else {
                continue;
            };
            if !endpoint_is_live(&endpoint.address) {
                let _ = std::fs::remove_file(&path);
                #[cfg(unix)]
                let _ = std::fs::remove_file(&endpoint.address);
            }
        }
    }
}

#[cfg(unix)]
fn endpoint_is_live(address: &str) -> bool {
    std::os::unix::net::UnixStream::connect(address).is_ok()
}

#[cfg(windows)]
fn endpoint_is_live(address: &str) -> bool {
    exchange_bytes(address, b"{}").is_ok()
}

/// Owner-side client used by the GUI. The session is deliberately distinct from
/// both the owner token and the agent token.
pub struct Client {
    endpoint: agent::Endpoint,
    session: String,
    pub state: HostState,
}

impl Client {
    pub fn connect(instance: &str) -> io::Result<Self> {
        let path = prefs::config_dir()
            .ok_or_else(|| io::Error::other("configuration directory unavailable"))?
            .join("host")
            .join(format!("{instance}.json"));
        let endpoint: agent::Endpoint = serde_json::from_slice(&std::fs::read(path)?)?;
        if endpoint.instance_id != instance
            || endpoint.protocol_version != document::PROTOCOL_VERSION
        {
            return Err(io::Error::other("host endpoint mismatch"));
        }
        let value = exchange(
            &endpoint.address,
            &serde_json::json!({"command":"gui_attach","token":endpoint.token}),
        )?;
        let session = value
            .get("session")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("host did not provide a GUI session"))?
            .to_owned();
        let state: HostState = serde_json::from_value(
            value
                .get("state")
                .cloned()
                .ok_or_else(|| io::Error::other("host state missing"))?,
        )?;
        Ok(Self {
            endpoint,
            session,
            state,
        })
    }

    pub fn spawn(path: Option<&std::path::Path>, mode: AgentMode) -> io::Result<Self> {
        use io::BufRead as _;
        let current = std::env::current_exe()?;
        let name = if cfg!(windows) {
            "ravnpad-host.exe"
        } else {
            "ravnpad-host"
        };
        let sibling = current.with_file_name(name);
        #[cfg(target_os = "macos")]
        let exe = if sibling.exists() {
            sibling
        } else {
            current
                .parent()
                .and_then(std::path::Path::parent)
                .unwrap_or(std::path::Path::new("."))
                .join("Helpers")
                .join(name)
        };
        #[cfg(not(target_os = "macos"))]
        let exe = sibling;
        let mut command = std::process::Command::new(exe);
        command.arg("start").arg("--agent").arg(mode.name());
        if let Some(path) = path {
            command.arg("--path").arg(path);
        }
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        let mut child = command.spawn()?;
        let mut line = String::new();
        io::BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("host stdout missing"))?,
        )
        .read_line(&mut line)?;
        let value: serde_json::Value = serde_json::from_str(&line)?;
        let instance = value
            .get("instance_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("host did not report its instance ID"))?;
        Self::connect(instance)
    }

    fn command(
        &mut self,
        command: &str,
        fields: serde_json::Value,
    ) -> io::Result<serde_json::Value> {
        let mut value = fields;
        value["command"] = command.into();
        value["token"] = self.endpoint.token.clone().into();
        value["session"] = self.session.clone().into();
        let response = exchange(&self.endpoint.address, &value)?;
        if let Some(state) = response.get("state") {
            self.state = serde_json::from_value(state.clone())?;
        }
        Ok(response)
    }

    pub fn refresh(&mut self) -> io::Result<&HostState> {
        self.command("gui_state", serde_json::json!({}))?;
        Ok(&self.state)
    }

    pub fn edit(&mut self, text: &str) -> io::Result<&HostState> {
        self.command(
            "gui_edit",
            serde_json::json!({"base_revision":self.state.identity.revision,"text":text}),
        )?;
        Ok(&self.state)
    }

    pub fn undo(&mut self) -> io::Result<&HostState> {
        self.command(
            "gui_undo",
            serde_json::json!({"base_revision":self.state.identity.revision}),
        )?;
        Ok(&self.state)
    }

    pub fn redo(&mut self) -> io::Result<&HostState> {
        self.command(
            "gui_redo",
            serde_json::json!({"base_revision":self.state.identity.revision}),
        )?;
        Ok(&self.state)
    }

    pub fn save(&mut self) -> io::Result<Option<String>> {
        let response = self.command(
            "gui_save",
            serde_json::json!({"base_revision":self.state.identity.revision}),
        )?;
        self.refresh()?;
        Ok(response
            .get("durability_warning")
            .and_then(|v| v.as_str())
            .map(str::to_owned))
    }

    pub fn save_as(&mut self, path: &std::path::Path) -> io::Result<Option<String>> {
        let response = self.command(
            "gui_save_as",
            serde_json::json!({"base_revision":self.state.identity.revision,"path":path}),
        )?;
        self.refresh()?;
        Ok(response
            .get("durability_warning")
            .and_then(|v| v.as_str())
            .map(str::to_owned))
    }

    pub fn recover(&mut self, path: &std::path::Path) -> io::Result<&HostState> {
        self.command("gui_recover", serde_json::json!({"path":path}))?;
        Ok(&self.state)
    }

    pub fn set_mode(&mut self, mode: AgentMode) -> io::Result<()> {
        self.command("host_set_mode", serde_json::json!({"mode":mode.name()}))?;
        Ok(())
    }

    pub fn detach(&mut self) -> io::Result<()> {
        self.command("gui_detach", serde_json::json!({}))
            .map(|_| ())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

fn exchange(address: &str, request: &serde_json::Value) -> io::Result<serde_json::Value> {
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() > agent::MAX_MESSAGE {
        return Err(io::Error::other("request exceeds transport limit"));
    }
    let response = exchange_bytes(address, &bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&response)?;
    if value.get("status").and_then(|v| v.as_str()) == Some("error") {
        let message = value
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("host command failed");
        let code = value.pointer("/error/code").and_then(|v| v.as_str());
        return Err(io::Error::new(
            match code {
                Some("stale_revision") => io::ErrorKind::WouldBlock,
                Some("unauthorized") => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            },
            message.to_owned(),
        ));
    }
    Ok(value)
}

#[cfg(unix)]
fn exchange_bytes(address: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    use io::{Read as _, Write as _};
    use std::{net::Shutdown, os::unix::net::UnixStream};
    let mut stream = UnixStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(bytes)?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    stream
        .take(agent::MAX_MESSAGE as u64 + 1)
        .read_to_end(&mut response)?;
    if response.len() > agent::MAX_MESSAGE {
        return Err(io::Error::other("host response exceeds transport limit"));
    }
    Ok(response)
}

#[cfg(windows)]
fn exchange_bytes(address: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt as _;
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut std::ffi::c_void,
            creation: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn SetNamedPipeHandleState(
            pipe: Handle,
            mode: *const u32,
            max_collection: *const u32,
            timeout: *const u32,
        ) -> i32;
        fn WriteFile(
            file: Handle,
            buffer: *const u8,
            size: u32,
            written: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn ReadFile(
            file: Handle,
            buffer: *mut u8,
            size: u32,
            read: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    let name: Vec<u16> = std::ffi::OsStr::new(address)
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { WaitNamedPipeW(name.as_ptr(), 5_000) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let pipe = unsafe {
        CreateFileW(
            name.as_ptr(),
            0xC0000000,
            0,
            std::ptr::null_mut(),
            3,
            0,
            std::ptr::null_mut(),
        )
    };
    if pipe == -1_isize as Handle {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mode = 2_u32;
        if unsafe { SetNamedPipeHandleState(pipe, &mode, std::ptr::null(), std::ptr::null()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut written = 0;
        if unsafe {
            WriteFile(
                pipe,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut result = Vec::new();
        loop {
            let mut chunk = [0_u8; 8192];
            let mut read = 0;
            let ok = unsafe {
                ReadFile(
                    pipe,
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            } != 0;
            result.extend_from_slice(&chunk[..read as usize]);
            if result.len() > agent::MAX_MESSAGE {
                return Err(io::Error::other("host response exceeds transport limit"));
            }
            if ok {
                break;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(234) {
                return Err(error);
            }
        }
        Ok(result)
    })();
    unsafe {
        CloseHandle(pipe);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_gui_session_can_be_replaced_without_sharing_edit_access() {
        let mut core = Core::new(None, AgentMode::Off).unwrap();
        let first = core.handle(
            agent::Request::GuiAttach {
                token: "owner".into(),
            },
            "owner",
            "agent",
        );
        let agent::Response::Gui {
            session: Some(first),
            ..
        } = first
        else {
            panic!("first GUI should attach");
        };
        assert!(matches!(
            core.handle(
                agent::Request::GuiAttach {
                    token: "owner".into()
                },
                "owner",
                "agent"
            ),
            agent::Response::Error { .. }
        ));
        core.gui_last_seen = Some(Instant::now() - GUI_SESSION_TIMEOUT);
        let next = core.handle(
            agent::Request::GuiAttach {
                token: "owner".into(),
            },
            "owner",
            "agent",
        );
        let agent::Response::Gui {
            session: Some(next),
            ..
        } = next
        else {
            panic!("expired GUI session should be replaced");
        };
        assert_ne!(first, next);
        assert!(matches!(
            core.handle(
                agent::Request::GuiEdit {
                    token: "owner".into(),
                    session: first,
                    base_revision: 0,
                    text: "old client".into(),
                },
                "owner",
                "agent",
            ),
            agent::Response::Error { .. }
        ));
        assert!(matches!(
            core.handle(
                agent::Request::GuiEdit {
                    token: "owner".into(),
                    session: next,
                    base_revision: 0,
                    text: "new client".into(),
                },
                "owner",
                "agent",
            ),
            agent::Response::Gui { .. }
        ));
        core.recovery.finish();
    }
}

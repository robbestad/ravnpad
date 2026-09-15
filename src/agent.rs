//! Local, opt-in transport for live document operations.

use crate::document::{self, Identity, Patch, Proposal, Snapshot};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

const MAX_MESSAGE: usize = 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    DocumentStatus {
        token: String,
    },
    DocumentRead {
        token: String,
        document_id: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    DocumentPropose {
        token: String,
        patch: Patch,
    },
}

impl Request {
    pub fn token(&self) -> &str {
        match self {
            Self::DocumentStatus { token }
            | Self::DocumentRead { token, .. }
            | Self::DocumentPropose { token, .. } => token,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Document { document: Identity },
    Ok { snapshot: Snapshot },
    PendingApproval { proposal: ProposalReceipt },
    Applied { proposal: ProposalReceipt },
    Rejected { proposal: ProposalReceipt },
    Error { error: ApiError },
}

/// Bounded transport representation of a proposal.
///
/// Pending proposals retain their complete before/after documents for the
/// approval UI. Returning them over IPC would duplicate the document and can
/// exceed the one-MiB transport limit even when the request itself is small.
#[derive(Clone, Debug, Serialize)]
pub struct ProposalReceipt {
    pub proposal_id: String,
    pub operation_id: String,
    pub document_id: String,
    pub base_revision: u64,
    pub base_hash: String,
    pub before_hash: String,
    pub after_hash: String,
    pub before_bytes: usize,
    pub after_bytes: usize,
}

impl From<&Proposal> for ProposalReceipt {
    fn from(proposal: &Proposal) -> Self {
        Self {
            proposal_id: proposal.proposal_id.clone(),
            operation_id: proposal.operation_id.clone(),
            document_id: proposal.document_id.clone(),
            base_revision: proposal.base_revision,
            base_hash: proposal.base_hash.clone(),
            before_hash: proposal.before_hash.clone(),
            after_hash: proposal.after_hash.clone(),
            before_bytes: proposal.before_bytes,
            after_bytes: proposal.after_bytes,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn document(error: document::Error) -> Self {
        let code = match error {
            document::Error::WrongDocument => "wrong_document",
            document::Error::StaleRevision { .. } => "stale_revision",
            document::Error::HashMismatch => "hash_mismatch",
            document::Error::EmptyPatch => "empty_patch",
            document::Error::TooManyEdits => "too_many_edits",
            document::Error::InvalidRange { .. } => "invalid_range",
            document::Error::InvalidUtf8Boundary { .. } => "invalid_utf8_boundary",
            document::Error::ExpectedTextMismatch { .. } => "expected_text_mismatch",
            document::Error::OverlappingEdits { .. } => "overlapping_edits",
            document::Error::AmbiguousInsertion { .. } => "ambiguous_insertion",
            document::Error::MixedLineEndings => "mixed_line_endings",
            document::Error::EmbeddedNul => "embedded_nul",
            document::Error::ResultTooLarge => "result_too_large",
            document::Error::InvalidOperationId => "invalid_operation_id",
            document::Error::OperationIdReused => "operation_id_reused",
            document::Error::ProposalHistoryFull => "proposal_history_full",
            document::Error::ProposalMemoryFull => "proposal_memory_full",
            document::Error::ProposalNotFound => "proposal_not_found",
            document::Error::ProposalRejected => "proposal_rejected",
            document::Error::ProposalAlreadyApplied => "proposal_already_applied",
        };
        Self::new(code, error.to_string())
    }
}

/// Clamp a snapshot to the exact serialized transport limit.
///
/// JSON escaping can expand a text slice by up to six bytes per input byte, so
/// bounding the requested raw byte range alone is insufficient.
pub fn bounded_snapshot_response(mut snapshot: Snapshot) -> Response {
    if snapshot_response_len(&snapshot) <= MAX_MESSAGE {
        return Response::Ok { snapshot };
    }

    snapshot.content_complete = false;
    snapshot
        .capabilities
        .retain(|capability| *capability != "propose");
    loop {
        let length = snapshot_response_len(&snapshot);
        if length <= MAX_MESSAGE {
            break;
        }
        if snapshot.text.is_empty() {
            return Response::Error {
                error: ApiError::new("response_too_large", "snapshot metadata exceeds one MiB"),
            };
        }
        let mut end =
            ((snapshot.text.len() as u128 * MAX_MESSAGE as u128) / length as u128) as usize;
        end = end.min(snapshot.text.len() - 1);
        while end > 0 && !snapshot.text.is_char_boundary(end) {
            end -= 1;
        }
        snapshot.text.truncate(end);
        snapshot.buffer_hash = document::hash(&snapshot.text);
    }
    Response::Ok { snapshot }
}

fn snapshot_response_len(snapshot: &Snapshot) -> usize {
    #[derive(Serialize)]
    struct SnapshotResponse<'a> {
        status: &'static str,
        snapshot: &'a Snapshot,
    }
    struct CountingWriter(usize);
    impl io::Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = CountingWriter(0);
    serde_json::to_writer(
        &mut writer,
        &SnapshotResponse {
            status: "ok",
            snapshot,
        },
    )
    .map(|_| writer.0)
    .unwrap_or(usize::MAX)
}

pub struct HostRequest {
    pub request: Request,
    response: Sender<Response>,
}

impl HostRequest {
    pub fn respond(self, response: Response) {
        let _ = self.response.send(response);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol_version: u32,
    pub instance_id: String,
    pub address: String,
    pub token: String,
}

pub struct Server {
    token: String,
    endpoint_path: PathBuf,
    rx: Receiver<HostRequest>,
}

impl Server {
    pub fn start(instance_id: &str, ctx: eframe::egui::Context) -> io::Result<Self> {
        let directory = crate::prefs::config_dir()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "configuration directory unavailable",
                )
            })?
            .join("agent");
        std::fs::create_dir_all(&directory)?;
        restrict_directory(&directory)?;
        let token = secure_token()?;
        let address = endpoint_address(&directory, instance_id);
        let endpoint = Endpoint {
            protocol_version: document::PROTOCOL_VERSION,
            instance_id: instance_id.to_owned(),
            address: address.clone(),
            token: token.clone(),
        };
        let endpoint_path = directory.join(format!("{instance_id}.json"));
        write_endpoint(&endpoint_path, &endpoint)?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ravnpad-agent".into())
            .spawn(move || listen(&address, tx, ctx))?;
        Ok(Self {
            token,
            endpoint_path,
            rx,
        })
    }

    pub fn try_recv(&self) -> Result<HostRequest, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn authorizes(&self, request: &Request) -> bool {
        constant_time_eq(self.token.as_bytes(), request.token().as_bytes())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.endpoint_path);
    }
}

fn write_endpoint(path: &std::path::Path, endpoint: &Endpoint) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, endpoint).map_err(io::Error::other)?;
    use std::io::Write as _;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn restrict_directory(_path: &std::path::Path) -> io::Result<()> {
    // The endpoint is under the user's roaming AppData directory. The named pipe
    // itself additionally uses an owner-only ACL and rejects remote clients.
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= left.get(index).copied().unwrap_or(0) as usize
            ^ right.get(index).copied().unwrap_or(0) as usize;
    }
    difference == 0
}

fn secure_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    fill_random(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read as _;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            size: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        Err(io::Error::other(format!(
            "BCryptGenRandom failed: 0x{status:08x}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn endpoint_address(_directory: &std::path::Path, instance_id: &str) -> String {
    format!(r"\\.\pipe\ravnpad-{instance_id}")
}

#[cfg(unix)]
fn endpoint_address(directory: &std::path::Path, instance_id: &str) -> String {
    directory
        .join(format!("{instance_id}.sock"))
        .to_string_lossy()
        .into_owned()
}

fn dispatch(bytes: &[u8], tx: &Sender<HostRequest>, ctx: &eframe::egui::Context) -> Response {
    if bytes.len() > MAX_MESSAGE {
        return Response::Error {
            error: ApiError::new("message_too_large", "request exceeds one MiB"),
        };
    }
    let request = match serde_json::from_slice(bytes) {
        Ok(request) => request,
        Err(error) => {
            return Response::Error {
                error: ApiError::new("invalid_request", error.to_string()),
            };
        }
    };
    let (response_tx, response_rx) = mpsc::channel();
    if tx
        .send(HostRequest {
            request,
            response: response_tx,
        })
        .is_err()
    {
        return Response::Error {
            error: ApiError::new("host_unavailable", "document host stopped"),
        };
    }
    ctx.request_repaint();
    response_rx
        .recv_timeout(RESPONSE_TIMEOUT)
        .unwrap_or_else(|_| Response::Error {
            error: ApiError::new("host_timeout", "document host did not respond in time"),
        })
}

#[cfg(unix)]
fn listen(address: &str, tx: Sender<HostRequest>, ctx: eframe::egui::Context) {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixListener;
    let _ = std::fs::remove_file(address);
    let Ok(listener) = UnixListener::bind(address) else {
        return;
    };
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut bytes = Vec::new();
        match (&mut stream)
            .take((MAX_MESSAGE + 1) as u64)
            .read_to_end(&mut bytes)
        {
            Ok(_) => {
                let response = dispatch(&bytes, &tx, &ctx);
                if let Ok(encoded) = serde_json::to_vec(&response) {
                    let _ = stream.write_all(&encoded);
                }
            }
            Err(_) => {}
        }
    }
}

#[cfg(windows)]
fn listen(address: &str, tx: Sender<HostRequest>, ctx: eframe::egui::Context) {
    use std::os::windows::ffi::OsStrExt as _;
    type Handle = *mut std::ffi::c_void;
    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut std::ffi::c_void,
        inherit: i32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_size: u32,
            in_size: u32,
            timeout: u32,
            security: *mut SecurityAttributes,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut std::ffi::c_void) -> i32;
        fn ReadFile(
            file: Handle,
            buffer: *mut u8,
            size: u32,
            read: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn WriteFile(
            file: Handle,
            buffer: *const u8,
            size: u32,
            written: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn FlushFileBuffers(file: Handle) -> i32;
        fn DisconnectNamedPipe(pipe: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
    }
    const INVALID_HANDLE: Handle = -1_isize as Handle;
    const PIPE_ACCESS_DUPLEX: u32 = 3;
    const PIPE_TYPE_MESSAGE: u32 = 4;
    const PIPE_READMODE_MESSAGE: u32 = 2;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 8;
    const ERROR_PIPE_CONNECTED: i32 = 535;
    let name: Vec<u16> = std::ffi::OsStr::new(address)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let sddl: Vec<u16> = std::ffi::OsStr::new("D:P(A;;GA;;;OW)")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return;
    }
    let mut security = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor,
        inherit: 0,
    };
    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                MAX_MESSAGE as u32,
                MAX_MESSAGE as u32,
                5_000,
                &mut security,
            )
        };
        if pipe == INVALID_HANDLE {
            break;
        }
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
            || io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_CONNECTED);
        if connected {
            let mut bytes = vec![0_u8; MAX_MESSAGE + 1];
            let mut read = 0;
            if unsafe {
                ReadFile(
                    pipe,
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            } != 0
            {
                bytes.truncate(read as usize);
                let response = dispatch(&bytes, &tx, &ctx);
                if let Ok(encoded) = serde_json::to_vec(&response) {
                    let mut written = 0;
                    unsafe {
                        WriteFile(
                            pipe,
                            encoded.as_ptr(),
                            encoded.len() as u32,
                            &mut written,
                            std::ptr::null_mut(),
                        );
                        FlushFileBuffers(pipe);
                    }
                }
            }
        }
        unsafe {
            DisconnectNamedPipe(pipe);
            CloseHandle(pipe);
        }
    }
    unsafe {
        LocalFree(descriptor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_checks_length_and_contents() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrex"));
        assert!(!constant_time_eq(b"secret", b"secret-long"));
    }

    #[test]
    fn request_has_stable_tagged_json_shape() {
        let request: Request = serde_json::from_str(
            r#"{"command":"document_read","token":"t","document_id":"d","offset":0,"limit":5}"#,
        )
        .unwrap();
        assert!(matches!(
            request,
            Request::DocumentRead { limit: Some(5), .. }
        ));
    }

    #[test]
    fn proposal_receipt_does_not_embed_large_documents() {
        let proposal = Proposal {
            proposal_id: "proposal-1".into(),
            operation_id: "operation-1".into(),
            document_id: "document-1".into(),
            base_revision: 7,
            base_hash: document::hash("before"),
            before_hash: document::hash(&"a".repeat(600 * 1024)),
            after_hash: document::hash(&"b".repeat(600 * 1024)),
            before_bytes: 600 * 1024,
            after_bytes: 600 * 1024,
            hunks: Vec::new(),
            before: "a".repeat(600 * 1024),
            after: "b".repeat(600 * 1024),
        };
        let response = Response::PendingApproval {
            proposal: (&proposal).into(),
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert!(encoded.len() < 1024);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&encoded).unwrap()["proposal"]["before_bytes"],
            600 * 1024
        );
    }

    #[test]
    fn snapshot_response_accounts_for_json_escaping() {
        let text = "\u{1}".repeat(MAX_MESSAGE);
        let document = document::Document::new(&text);
        let snapshot = document.snapshot(
            &text,
            None,
            false,
            document::ExternalState::Unknown,
            true,
            true,
            false,
        );
        let response = bounded_snapshot_response(snapshot);
        let encoded = serde_json::to_vec(&response).unwrap();
        assert!(encoded.len() <= MAX_MESSAGE);
        let Response::Ok { snapshot } = response else {
            panic!("bounded snapshot should fit in the transport");
        };
        assert!(snapshot.text.len() < text.len());
        assert!(!snapshot.content_complete);
        assert_eq!(snapshot.buffer_hash, document::hash(&snapshot.text));
    }
}

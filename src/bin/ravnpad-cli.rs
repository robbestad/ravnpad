use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(unix)]
use std::io::Write as _;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

const MAX_MESSAGE: usize = 1024 * 1024;
const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Deserialize)]
struct Endpoint {
    protocol_version: u32,
    instance_id: String,
    address: String,
    token: String,
}

#[derive(Serialize)]
struct FileRead<'a> {
    protocol_version: u32,
    path: &'a Path,
    content_complete: bool,
    range_unit: &'static str,
    text: &'a str,
}

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}

#[derive(Debug)]
struct CliError {
    code: i32,
    message: String,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: 3,
            message: message.into(),
        }
    }
    fn protocol(message: impl Into<String>) -> Self {
        Self {
            code: 4,
            message: message.into(),
        }
    }
    fn exit_code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn run() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [family, command, path, rest @ ..] if family == "file" && command == "read" => {
            if rest.iter().any(|arg| arg != "--json") {
                return Err(CliError::usage(usage()));
            }
            let text = std::fs::read_to_string(path)
                .map_err(|error| CliError::unavailable(format!("cannot read {path}: {error}")))?;
            print_json(&FileRead {
                protocol_version: PROTOCOL_VERSION,
                path: Path::new(path),
                content_complete: true,
                range_unit: "utf8-byte",
                text: &text,
            })
        }
        [family, command, rest @ ..] if family == "document" && command == "status" => {
            let instance = option(rest, "--instance")?;
            ensure_known(rest, &["--instance", "--json"])?;
            let endpoint = endpoint(instance)?;
            ensure_instance(&endpoint, instance)?;
            send(
                &endpoint.address,
                &json!({
                    "command": "document_status",
                    "token": endpoint.token,
                }),
            )
        }
        [family, command, rest @ ..] if family == "document" && command == "read" => {
            let instance = option(rest, "--instance")?;
            let document = option(rest, "--document")?;
            let offset = optional_usize(rest, "--offset")?;
            let limit = optional_usize(rest, "--limit")?;
            ensure_known(
                rest,
                &["--instance", "--document", "--offset", "--limit", "--json"],
            )?;
            let endpoint = endpoint(instance)?;
            ensure_instance(&endpoint, instance)?;
            send(
                &endpoint.address,
                &json!({
                    "command": "document_read",
                    "token": endpoint.token,
                    "document_id": document,
                    "offset": offset,
                    "limit": limit,
                }),
            )
        }
        [family, command, rest @ ..] if family == "document" && command == "propose" => {
            let instance = option(rest, "--instance")?;
            let document = option(rest, "--document")?;
            ensure_known(rest, &["--instance", "--document", "--stdin", "--json"])?;
            if !rest.iter().any(|arg| arg == "--stdin") {
                return Err(CliError::usage("document propose requires --stdin"));
            }
            let mut input = String::new();
            io::stdin()
                .take((MAX_MESSAGE + 1) as u64)
                .read_to_string(&mut input)
                .map_err(|error| CliError::protocol(format!("cannot read stdin: {error}")))?;
            if input.len() > MAX_MESSAGE {
                return Err(CliError::protocol("patch exceeds one MiB"));
            }
            let mut patch: Value = serde_json::from_str(&input)
                .map_err(|error| CliError::protocol(format!("invalid patch JSON: {error}")))?;
            if patch.get("document_id").is_none() {
                patch["document_id"] = Value::String(document.to_owned());
            }
            if patch.get("document_id").and_then(Value::as_str) != Some(document) {
                return Err(CliError::usage("patch document_id differs from --document"));
            }
            let endpoint = endpoint(instance)?;
            ensure_instance(&endpoint, instance)?;
            send(
                &endpoint.address,
                &json!({ "command": "document_propose", "token": endpoint.token, "patch": patch }),
            )
        }
        _ => Err(CliError::usage(usage())),
    }
}

fn usage() -> String {
    "usage:\n  ravnpad-cli file read PATH --json\n  ravnpad-cli document status --instance ID --json\n  ravnpad-cli document read --instance ID --document ID [--offset N --limit N] --json\n  ravnpad-cli document propose --instance ID --document ID --stdin --json".into()
}

fn option<'a>(args: &'a [String], name: &str) -> Result<&'a str, CliError> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .ok_or_else(|| CliError::usage(format!("missing {name}")))
}

fn optional_usize(args: &[String], name: &str) -> Result<Option<usize>, CliError> {
    match args.windows(2).find(|pair| pair[0] == name) {
        Some(pair) => pair[1]
            .parse()
            .map(Some)
            .map_err(|_| CliError::usage(format!("{name} must be a non-negative integer"))),
        None => Ok(None),
    }
}

fn ensure_known(args: &[String], value_options: &[&str]) -> Result<(), CliError> {
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument == "--json" || argument == "--stdin" {
            index += 1;
            continue;
        }
        if value_options.contains(&argument) && index + 1 < args.len() {
            index += 2;
            continue;
        }
        return Err(CliError::usage(format!(
            "unknown or incomplete option: {argument}"
        )));
    }
    Ok(())
}

fn endpoint(instance: &str) -> Result<Endpoint, CliError> {
    let path = config_dir()
        .ok_or_else(|| CliError::unavailable("configuration directory unavailable"))?
        .join("agent")
        .join(format!("{instance}.json"));
    let bytes = std::fs::read(&path).map_err(|error| {
        CliError::unavailable(format!("RavnPad instance is unavailable: {error}"))
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CliError::protocol(format!("invalid endpoint metadata: {error}")))
}

fn ensure_instance(endpoint: &Endpoint, expected: &str) -> Result<(), CliError> {
    if endpoint.protocol_version != PROTOCOL_VERSION {
        return Err(CliError::protocol("unsupported protocol version"));
    }
    if endpoint.instance_id != expected {
        return Err(CliError::protocol("endpoint instance mismatch"));
    }
    Ok(())
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("RavnPad"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/RavnPad"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
        Some(base.join("ravnpad"))
    }
}

fn send(address: &str, request: &Value) -> Result<(), CliError> {
    let encoded =
        serde_json::to_vec(request).map_err(|error| CliError::protocol(error.to_string()))?;
    if encoded.len() > MAX_MESSAGE {
        return Err(CliError::protocol("request exceeds one MiB"));
    }
    let response = exchange(address, &encoded)?;
    let value: Value = serde_json::from_slice(&response)
        .map_err(|error| CliError::protocol(format!("invalid host response: {error}")))?;
    print_json(&value)?;
    if value.get("status").and_then(Value::as_str) == Some("error") {
        Err(CliError::protocol(
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("document operation failed"),
        ))
    } else {
        Ok(())
    }
}

fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer(io::stdout().lock(), value)
        .map_err(|error| CliError::protocol(error.to_string()))?;
    println!();
    Ok(())
}

#[cfg(unix)]
fn exchange(address: &str, request: &[u8]) -> Result<Vec<u8>, CliError> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(address)
        .map_err(|error| CliError::unavailable(format!("cannot connect to RavnPad: {error}")))?;
    stream
        .write_all(request)
        .map_err(|error| CliError::unavailable(error.to_string()))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| CliError::unavailable(error.to_string()))?;
    let mut response = Vec::new();
    stream
        .take((MAX_MESSAGE + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|error| CliError::unavailable(error.to_string()))?;
    if response.len() > MAX_MESSAGE {
        return Err(CliError::protocol("response exceeds one MiB"));
    }
    Ok(response)
}

#[cfg(windows)]
fn exchange(address: &str, request: &[u8]) -> Result<Vec<u8>, CliError> {
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
    const INVALID_HANDLE: Handle = -1_isize as Handle;
    let name: Vec<u16> = std::ffi::OsStr::new(address)
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { WaitNamedPipeW(name.as_ptr(), 5_000) } == 0 {
        return Err(CliError::unavailable(format!(
            "RavnPad pipe unavailable: {}",
            io::Error::last_os_error()
        )));
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
    if pipe == INVALID_HANDLE {
        return Err(CliError::unavailable(format!(
            "cannot connect to RavnPad: {}",
            io::Error::last_os_error()
        )));
    }
    let result = (|| {
        let mode = 2_u32;
        if unsafe { SetNamedPipeHandleState(pipe, &mode, std::ptr::null(), std::ptr::null()) } == 0
        {
            return Err(CliError::unavailable(
                io::Error::last_os_error().to_string(),
            ));
        }
        let mut written = 0;
        if unsafe {
            WriteFile(
                pipe,
                request.as_ptr(),
                request.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(CliError::unavailable(
                io::Error::last_os_error().to_string(),
            ));
        }
        let mut response = vec![0_u8; MAX_MESSAGE];
        let mut read = 0;
        if unsafe {
            ReadFile(
                pipe,
                response.as_mut_ptr(),
                response.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(CliError::unavailable(
                io::Error::last_os_error().to_string(),
            ));
        }
        response.truncate(read as usize);
        Ok(response)
    })();
    unsafe {
        CloseHandle(pipe);
    }
    result
}

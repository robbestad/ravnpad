use serde_json::{Value, json};
use std::{
    io::{BufRead as _, Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

struct Running {
    child: Child,
    root: tempfile::TempDir,
    instance: String,
    document: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn configure(command: &mut Command, root: &Path) {
    #[cfg(target_os = "macos")]
    {
        command.env("HOME", root);
    }
    #[cfg(windows)]
    {
        command.env("APPDATA", root);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        command.env("XDG_CONFIG_HOME", root);
    }
}

fn config_dir(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        root.join("Library/Application Support/RavnPad")
    }
    #[cfg(windows)]
    {
        root.join("RavnPad")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        root.join("ravnpad")
    }
}

fn start() -> (Running, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    std::fs::write(&path, "first").unwrap();
    let endpoints = config_dir(root.path()).join("host");
    std::fs::create_dir_all(&endpoints).unwrap();
    let stale = endpoints.join("stale.json");
    #[cfg(unix)]
    let stale_address = "/tmp/ravnpad-stale-integration.sock";
    #[cfg(windows)]
    let stale_address = r"\\.\pipe\ravnpad-stale-integration";
    std::fs::write(
        &stale,
        json!({"protocol_version":2,"instance_id":"stale","address":stale_address,"token":"stale"})
            .to_string(),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ravnpad-host"));
    configure(&mut command, root.path());
    let mut child = command
        .args(["serve", "--path"])
        .arg(&path)
        .args(["--agent", "edit"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(!line.is_empty(), "host failed to start");
    assert!(!stale.exists(), "stale endpoint was not cleaned up");
    let ids: Value = serde_json::from_str(&line).unwrap();
    (
        Running {
            instance: ids["instance_id"].as_str().unwrap().into(),
            document: ids["document_id"].as_str().unwrap().into(),
            child,
            root,
        },
        path,
    )
}

fn cli(host: &Running, args: &[&str], input: Option<&str>) -> (bool, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ravnpad-cli"));
    configure(&mut command, host.root.path());
    let mut child = command
        .args(args)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "CLI gave no JSON: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output.status.success(), value)
}

fn endpoint(host: &Running, kind: &str) -> Value {
    let path = config_dir(host.root.path())
        .join(kind)
        .join(format!("{}.json", host.instance));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[cfg(unix)]
fn raw(endpoint: &Value, request: Value) -> Value {
    use std::{net::Shutdown, os::unix::net::UnixStream};
    let mut stream = UnixStream::connect(endpoint["address"].as_str().unwrap()).unwrap();
    stream
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn headless_edits_save_conflict_and_stop() {
    let (host, path) = start();
    let instance = &host.instance;
    let document = &host.document;
    let (ok, status) = cli(&host, &["document", "status", "--instance", instance], None);
    assert!(ok);
    assert_eq!(status["document"]["document_id"], *document);
    let (ok, read) = cli(
        &host,
        &[
            "document",
            "read",
            "--instance",
            instance,
            "--document",
            document,
        ],
        None,
    );
    assert!(ok);
    assert_eq!(read["snapshot"]["text"], "first");
    let patch = json!({"operation_id":"integration-edit","document_id":document,"base_revision":read["snapshot"]["revision"],"base_hash":read["snapshot"]["buffer_hash"],"edits":[{"start_byte":0,"end_byte":5,"expected_text":"first","replacement":"second"}]}).to_string();
    let (ok, applied) = cli(
        &host,
        &[
            "document",
            "propose",
            "--instance",
            instance,
            "--document",
            document,
            "--stdin",
        ],
        Some(&patch),
    );
    assert!(ok, "{applied}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    let (ok, stale) = cli(
        &host,
        &[
            "document",
            "propose",
            "--instance",
            instance,
            "--document",
            document,
            "--stdin",
        ],
        Some(&patch.replace("integration-edit", "stale-edit")),
    );
    assert!(!ok);
    assert_eq!(stale["error"]["code"], "stale_revision");

    #[cfg(unix)]
    {
        let agent = endpoint(&host, "agent");
        let owner = endpoint(&host, "host");
        assert_eq!(
            raw(
                &owner,
                json!({"command":"host_stop","token":agent["token"],"discard":true})
            )["error"]["code"],
            "unauthorized"
        );
        assert_eq!(
            raw(
                &owner,
                json!({"command":"host_set_mode","token":agent["token"],"mode":"off"})
            )["error"]["code"],
            "unauthorized"
        );
        let attached = raw(
            &owner,
            json!({"command":"gui_attach","token":owner["token"]}),
        );
        let session = attached["session"].as_str().unwrap();
        assert_eq!(
            raw(
                &owner,
                json!({"command":"gui_attach","token":owner["token"]})
            )["error"]["code"],
            "gui_busy"
        );
        let edited = raw(
            &owner,
            json!({"command":"gui_edit","token":owner["token"],"session":session,"base_revision":1,"text":"third"}),
        );
        assert_eq!(edited["state"]["text"], "third");
        let revision = edited["state"]["identity"]["revision"].as_u64().unwrap();
        let depth = edited["state"]["undo_depth"].as_u64().unwrap();
        assert_eq!(
            raw(
                &owner,
                json!({"command":"gui_detach","token":owner["token"],"session":session})
            )["status"],
            "gui"
        );
        let attached = raw(
            &owner,
            json!({"command":"gui_attach","token":owner["token"]}),
        );
        assert_eq!(attached["state"]["identity"]["document_id"], *document);
        assert_eq!(attached["state"]["identity"]["revision"], revision);
        assert_eq!(attached["state"]["text"], "third");
        assert_eq!(attached["state"]["undo_depth"], depth);
        let session = attached["session"].as_str().unwrap();
        let undo = raw(
            &owner,
            json!({"command":"gui_undo","token":owner["token"],"session":session,"base_revision":revision}),
        );
        assert_eq!(undo["state"]["text"], "second");
        let revision = undo["state"]["identity"]["revision"].as_u64().unwrap();
        let redo = raw(
            &owner,
            json!({"command":"gui_redo","token":owner["token"],"session":session,"base_revision":revision}),
        );
        assert_eq!(redo["state"]["text"], "third");
        assert_eq!(
            raw(
                &owner,
                json!({"command":"host_stop","token":owner["token"],"discard":false})
            )["error"]["code"],
            "unsaved_changes"
        );
    }

    let (ok, saved) = cli(
        &host,
        &[
            "document",
            "save",
            "--instance",
            instance,
            "--document",
            document,
        ],
        None,
    );
    assert!(ok, "{saved}");
    #[cfg(unix)]
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "third");
    #[cfg(windows)]
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    std::fs::write(&path, "outside").unwrap();
    #[cfg(unix)]
    {
        let owner = endpoint(&host, "host");
        let session = raw(
            &owner,
            json!({"command":"gui_attach","token":owner["token"]}),
        );
        // The earlier editing GUI is still attached, so an external agent patch is used below.
        assert_eq!(session["error"]["code"], "gui_busy");
    }
    let (ok, read) = cli(
        &host,
        &[
            "document",
            "read",
            "--instance",
            instance,
            "--document",
            document,
        ],
        None,
    );
    assert!(ok);
    let text = read["snapshot"]["text"].as_str().unwrap();
    let patch = json!({"operation_id":"conflict-edit","document_id":document,"base_revision":read["snapshot"]["revision"],"base_hash":read["snapshot"]["buffer_hash"],"edits":[{"start_byte":0,"end_byte":text.len(),"expected_text":text,"replacement":"fourth"}]}).to_string();
    assert!(
        cli(
            &host,
            &[
                "document",
                "propose",
                "--instance",
                instance,
                "--document",
                document,
                "--stdin"
            ],
            Some(&patch)
        )
        .0
    );
    let (ok, conflict) = cli(
        &host,
        &[
            "document",
            "save",
            "--instance",
            instance,
            "--document",
            document,
        ],
        None,
    );
    assert!(!ok);
    assert_eq!(conflict["error"]["code"], "file_conflict");
    let (ok, stop) = cli(&host, &["host", "stop", "--instance", instance], None);
    assert!(!ok);
    assert_eq!(stop["error"]["code"], "unsaved_changes");
    assert!(
        cli(
            &host,
            &["host", "stop", "--instance", instance, "--discard"],
            None
        )
        .0
    );
    let recovery = config_dir(host.root.path()).join("recovery");
    for _ in 0..100 {
        if std::fs::read_dir(&recovery).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "txt"))
        }) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("discarded buffer did not leave a recovery copy");
}

#[cfg(unix)]
#[test]
fn pathless_document_needs_explicit_save_as() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ravnpad-host"));
    configure(&mut command, root.path());
    let mut child = command
        .args(["serve", "--agent", "edit"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(!line.is_empty());
    let ids: Value = serde_json::from_str(&line).unwrap();
    let host = Running {
        instance: ids["instance_id"].as_str().unwrap().into(),
        document: ids["document_id"].as_str().unwrap().into(),
        child,
        root,
    };
    let owner = endpoint(&host, "host");
    let attached = raw(
        &owner,
        json!({"command":"gui_attach","token":owner["token"]}),
    );
    let session = attached["session"].as_str().unwrap();
    let edited = raw(
        &owner,
        json!({"command":"gui_edit","token":owner["token"],"session":session,"base_revision":0,"text":"draft"}),
    );
    assert_eq!(edited["state"]["dirty"], true);
    let (ok, saved) = cli(
        &host,
        &[
            "document",
            "save",
            "--instance",
            &host.instance,
            "--document",
            &host.document,
        ],
        None,
    );
    assert!(!ok);
    assert_eq!(saved["error"]["code"], "save_failed");
    let path = host.root.path().join("draft.txt");
    let save_as = raw(
        &owner,
        json!({"command":"gui_save_as","token":owner["token"],"session":session,"path":path}),
    );
    assert_eq!(save_as["status"], "saved");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "draft");
    assert!(cli(&host, &["host", "stop", "--instance", &host.instance], None).0);
}

#[cfg(unix)]
#[test]
fn host_restores_and_clears_recovery_after_save() {
    let root = tempfile::tempdir().unwrap();
    let recovery_dir = config_dir(root.path()).join("recovery");
    std::fs::create_dir_all(&recovery_dir).unwrap();
    let copy = recovery_dir.join("old-draft.txt");
    std::fs::write(&copy, "restored æøå").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ravnpad-host"));
    configure(&mut command, root.path());
    let mut child = command
        .args(["serve", "--agent", "off"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let ids: Value = serde_json::from_str(&line).unwrap();
    let host = Running {
        instance: ids["instance_id"].as_str().unwrap().into(),
        document: ids["document_id"].as_str().unwrap().into(),
        child,
        root,
    };
    let owner = endpoint(&host, "host");
    let attached = raw(
        &owner,
        json!({"command":"gui_attach","token":owner["token"]}),
    );
    let session = attached["session"].as_str().unwrap();
    let restored = raw(
        &owner,
        json!({"command":"gui_recover","token":owner["token"],"session":session,"path":copy}),
    );
    assert_eq!(restored["state"]["text"], "restored æøå");
    assert_eq!(restored["state"]["dirty"], true);
    assert_ne!(restored["state"]["identity"]["document_id"], host.document);
    let path = host.root.path().join("saved.txt");
    let saved = raw(
        &owner,
        json!({"command":"gui_save_as","token":owner["token"],"session":session,"path":path}),
    );
    assert_eq!(saved["status"], "saved");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "restored æøå");
    for _ in 0..100 {
        if !copy.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!copy.exists(), "saved recovery copy was not cleared");
    assert!(cli(&host, &["host", "stop", "--instance", &host.instance], None).0);
}

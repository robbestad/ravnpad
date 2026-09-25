use std::{
    fs::{self, OpenOptions},
    io,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use fs2::FileExt as _;

pub enum Command {
    Snapshot(Option<String>),
    SnapshotChecked(String, Sender<io::Result<()>>),
    Read(PathBuf),
    Delete(PathBuf),
    Stop,
}
#[derive(Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub preview: String,
}
pub enum Event {
    Available(Vec<Candidate>),
    Restored(PathBuf, String),
    Failed(io::Error),
    ReadFailed(io::Error),
}
pub struct Recovery {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Recovery {
    pub fn start() -> Self {
        Self::start_with_wake(Arc::new(|| {}))
    }

    pub fn start_with_wake(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start_in_with_wake(crate::prefs::config_dir().map(|p| p.join("recovery")), wake)
    }

    pub(crate) fn start_in(dir: Option<PathBuf>) -> Self {
        Self::start_in_with_wake(dir, Arc::new(|| {}))
    }

    fn start_in_with_wake(dir: Option<PathBuf>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let Some(dir) = dir else {
                return;
            };
            if let Err(error) = fs::create_dir_all(&dir) {
                let _ = events.send(Event::Failed(error));
                wake();
                return;
            }
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = dir.join(format!("{}-{unique}.txt", std::process::id()));
            let lock_path = recovery_lock(&path);
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
                .and_then(|file| {
                    file.lock_exclusive()?;
                    Ok(file)
                });
            let lock = match lock {
                Ok(lock) => lock,
                Err(error) => {
                    let _ = events.send(Event::Failed(error));
                    wake();
                    return;
                }
            };
            let result = (|| {
                fs::read_dir(&dir)?
                    .map(|entry| entry.map(|e| e.path()))
                    .collect::<io::Result<Vec<_>>>()
            })();
            let event = match result {
                Ok(mut paths) => {
                    paths.retain(|p| {
                        p.extension().is_some_and(|e| e == "txt") && !recovery_is_live(p)
                    });
                    paths.sort();
                    use std::io::Read;
                    Event::Available(
                        paths
                            .into_iter()
                            .map(|path| {
                                let mut bytes = Vec::new();
                                if let Ok(file) = fs::File::open(&path) {
                                    let _ = file.take(512).read_to_end(&mut bytes);
                                }
                                let preview = String::from_utf8_lossy(&bytes)
                                    .lines()
                                    .find(|line| !line.trim().is_empty())
                                    .unwrap_or("…")
                                    .chars()
                                    .take(70)
                                    .collect();
                                Candidate { path, preview }
                            })
                            .collect(),
                    )
                }
                Err(e) => Event::Failed(e),
            };
            let _ = events.send(event);
            wake();
            while let Ok(command) = commands.recv() {
                let result = match command {
                    Command::Snapshot(Some(text)) => {
                        crate::storage::save(&path, text.as_bytes()).map(|_| None)
                    }
                    Command::SnapshotChecked(text, reply) => {
                        let result = crate::storage::save(&path, text.as_bytes()).map(|_| ());
                        let _ = reply.send(result);
                        Ok(None)
                    }
                    Command::Snapshot(None) => remove(&path).map(|_| None),
                    Command::Read(path) => Ok(Some(match fs::read_to_string(&path) {
                        Ok(text) => Event::Restored(path, text),
                        Err(e) => Event::ReadFailed(e),
                    })),
                    Command::Stop => break,
                    Command::Delete(path) => remove_recovery(&path).map(|_| None),
                };
                match result {
                    Ok(Some(event)) => {
                        let _ = events.send(event);
                        wake();
                    }
                    Err(e) => {
                        let _ = events.send(Event::Failed(e));
                        wake();
                    }
                    _ => {}
                }
            }
            let _ = fs2::FileExt::unlock(&lock);
            drop(lock);
            let _ = remove(&lock_path);
        });
        Self {
            tx,
            rx,
            worker: Some(worker),
        }
    }
}

fn recovery_lock(path: &std::path::Path) -> PathBuf {
    path.with_extension("lock")
}

fn recovery_is_live(path: &std::path::Path) -> bool {
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(recovery_lock(path))
    else {
        return false;
    };
    match lock.try_lock_exclusive() {
        Ok(()) => {
            let _ = fs2::FileExt::unlock(&lock);
            false
        }
        Err(_) => true,
    }
}

fn remove_recovery(path: &std::path::Path) -> io::Result<()> {
    remove(path)?;
    remove(&recovery_lock(path))
}
fn remove(path: &std::path::Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

impl Recovery {
    pub fn snapshot_checked(&self, text: String) -> io::Result<()> {
        let (reply, result) = mpsc::channel();
        self.tx
            .send(Command::SnapshotChecked(text, reply))
            .map_err(|_| io::Error::other("recovery worker unavailable"))?;
        result
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| io::Error::other("recovery snapshot not confirmed"))?
    }

    pub fn finish(&mut self) {
        // Cocoa termination may exit before Rust destructors run.
        let _ = self.tx.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Recovery {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_wakes_gui_recovery_poll() {
        let dir = tempfile::tempdir().unwrap();
        let (wake_tx, wake_rx) = mpsc::channel();
        let worker = Recovery::start_in_with_wake(
            Some(dir.path().to_owned()),
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
        );
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Available(_)
        ));
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        worker
            .tx
            .send(Command::Read(dir.path().join("missing.txt")))
            .unwrap();
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::ReadFailed(_)
        ));
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    fn available(worker: &Recovery) -> Vec<PathBuf> {
        match worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        {
            Event::Available(paths) => paths.into_iter().map(|c| c.path).collect(),
            _ => panic!("expected recovery list"),
        }
    }
    #[test]
    fn snapshot_survives_restart_and_restores_exact_unicode() {
        let dir = tempfile::tempdir().unwrap();

        {
            let worker = Recovery::start_in(Some(dir.path().to_owned()));
            assert!(available(&worker).is_empty());
            worker
                .tx
                .send(Command::Snapshot(Some("æøå\nunsaved".into())))
                .unwrap();
        }
        let worker = Recovery::start_in(Some(dir.path().to_owned()));
        let paths = available(&worker);
        assert_eq!(paths.len(), 1);
        worker.tx.send(Command::Read(paths[0].clone())).unwrap();
        match worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        {
            Event::Restored(_, text) => assert_eq!(text, "æøå\nunsaved"),
            _ => panic!("expected restored text"),
        }
    }

    #[test]
    fn snapshots_from_a_live_window_are_not_offered() {
        let dir = tempfile::tempdir().unwrap();
        let live = Recovery::start_in(Some(dir.path().to_owned()));
        assert!(available(&live).is_empty());
        live.tx
            .send(Command::Snapshot(Some("still being edited".into())))
            .unwrap();
        for _ in 0..100 {
            if fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "txt"))
            {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let worker = Recovery::start_in(Some(dir.path().to_owned()));
        assert!(available(&worker).is_empty());
    }
    #[test]
    fn cleanup_is_ordered_after_pending_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        {
            let worker = Recovery::start_in(Some(dir.path().to_owned()));
            available(&worker);
            worker
                .tx
                .send(Command::Snapshot(Some("draft".into())))
                .unwrap();
            worker.tx.send(Command::Snapshot(None)).unwrap();
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

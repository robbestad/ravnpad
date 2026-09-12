use std::{
    fs, io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub enum Command {
    Snapshot(Option<String>),
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
    pub fn start(ctx: eframe::egui::Context) -> Self {
        Self::start_in(ctx, crate::prefs::config_dir().map(|p| p.join("recovery")))
    }

    pub(crate) fn start_in(ctx: eframe::egui::Context, dir: Option<PathBuf>) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let Some(dir) = dir else {
                return;
            };
            let result = fs::create_dir_all(&dir).and_then(|_| {
                fs::read_dir(&dir)?
                    .map(|entry| entry.map(|e| e.path()))
                    .collect::<io::Result<Vec<_>>>()
            });
            let event = match result {
                Ok(mut paths) => {
                    paths.retain(|p| p.extension().is_some_and(|e| e == "txt"));
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
            ctx.request_repaint();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = dir.join(format!("{}-{unique}.txt", std::process::id()));
            while let Ok(command) = commands.recv() {
                let result = match command {
                    Command::Snapshot(Some(text)) => {
                        crate::storage::save(&path, text.as_bytes()).map(|_| None)
                    }
                    Command::Snapshot(None) => remove(&path).map(|_| None),
                    Command::Read(path) => Ok(Some(match fs::read_to_string(&path) {
                        Ok(text) => Event::Restored(path, text),
                        Err(e) => Event::ReadFailed(e),
                    })),
                    Command::Stop => break,
                    Command::Delete(path) => remove(&path).map(|_| None),
                };
                match result {
                    Ok(Some(event)) => {
                        let _ = events.send(event);
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        let _ = events.send(Event::Failed(e));
                        ctx.request_repaint();
                    }
                    _ => {}
                }
            }
        });
        Self {
            tx,
            rx,
            worker: Some(worker),
        }
    }
}
fn remove(path: &std::path::Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        // Drain earlier snapshot/deletion jobs before a clean process exit.
        let _ = self.tx.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let ctx = eframe::egui::Context::default();
        {
            let worker = Recovery::start_in(ctx.clone(), Some(dir.path().to_owned()));
            assert!(available(&worker).is_empty());
            worker
                .tx
                .send(Command::Snapshot(Some("æøå\nunsaved".into())))
                .unwrap();
        }
        let worker = Recovery::start_in(ctx, Some(dir.path().to_owned()));
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
    fn cleanup_is_ordered_after_pending_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        {
            let worker = Recovery::start_in(
                eframe::egui::Context::default(),
                Some(dir.path().to_owned()),
            );
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

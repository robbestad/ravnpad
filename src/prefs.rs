use std::path::PathBuf;

use crate::i18n::{self, Lang};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

impl ThemePref {
    #[cfg_attr(test, allow(dead_code))]
    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_code(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Prefs {
    pub lang: Lang,
    pub font: String,
    pub size: f32,
    pub spellcheck: bool,
    pub theme: ThemePref,
    pub line_wrap: bool,
    pub recent: Vec<PathBuf>,
    pub(crate) positions: Vec<(PathBuf, f32, u64)>,
}

impl Prefs {
    pub fn load() -> Self {
        let mut prefs = Self {
            lang: i18n::detect(),
            font: String::new(),
            size: 15.0,
            spellcheck: false,
            theme: ThemePref::System,
            line_wrap: true,
            recent: Vec::new(),
            positions: Vec::new(),
        };
        if let Some(path) = settings_path()
            && let Ok(text) = std::fs::read_to_string(path)
        {
            for line in text.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    match key.trim() {
                        "lang" => {
                            if let Some(lang) = Lang::from_code(value.trim()) {
                                prefs.lang = lang;
                            }
                        }
                        "font" => prefs.font = value.trim().to_owned(),
                        "spell" => prefs.spellcheck = value.trim() == "1",
                        "theme" => {
                            if let Some(theme) = ThemePref::from_code(value.trim()) {
                                prefs.theme = theme;
                            }
                        }
                        "wrap" => prefs.line_wrap = value.trim() != "0",
                        "recent" => {
                            prefs.recent = decode_recent(value.trim());
                        }
                        "positions" => {
                            prefs.positions = decode_positions(value.trim());
                        }
                        "size" => {
                            if let Ok(size) = value.trim().parse::<f32>()
                                && size.is_finite()
                            {
                                prefs.size = size.clamp(6.0, 144.0);
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else if let Some(path) = language_path()
            && let Ok(code) = std::fs::read_to_string(path)
            && let Some(lang) = Lang::from_code(code.trim())
        {
            prefs.lang = lang;
        }
        prefs.recent.truncate(10);
        prefs.positions.truncate(50);
        prefs
    }

    #[cfg_attr(test, allow(dead_code))]
    pub fn save(&self) -> Result<(), std::io::Error> {
        let Some(path) = settings_path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = format!(
            "lang={}\nfont={}\nsize={}\nspell={}\ntheme={}\nwrap={}\nrecent={}\npositions={}\n",
            self.lang.code(),
            self.font,
            self.size,
            u8::from(self.spellcheck),
            self.theme.code(),
            u8::from(self.line_wrap),
            encode_recent(&self.recent),
            encode_positions(&self.positions),
        );
        crate::storage::save(&path, body.as_bytes()).map(|_| ())
    }

    pub fn add_recent(&mut self, path: &std::path::Path) {
        self.recent.retain(|item| item != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(10);
    }

    pub fn position(&self, path: &std::path::Path) -> (f32, u64) {
        self.positions
            .iter()
            .find(|(item, _, _)| item == path)
            .map(|(_, scroll, offset)| (*scroll, *offset))
            .unwrap_or_default()
    }

    pub fn set_position(&mut self, path: &std::path::Path, scroll: f32, offset: u64) {
        self.positions.retain(|(item, _, _)| item != path);
        self.positions
            .insert(0, (path.to_path_buf(), scroll.max(0.0), offset));
        self.positions.truncate(50);
    }
}

fn encode_recent(paths: &[PathBuf]) -> String {
    let values: Vec<_> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_owned())
}

fn decode_recent(value: &str) -> Vec<PathBuf> {
    serde_json::from_str::<Vec<String>>(value)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn encode_positions(positions: &[(PathBuf, f32, u64)]) -> String {
    let values: Vec<_> = positions
        .iter()
        .map(|(path, scroll, offset)| (path.to_string_lossy().into_owned(), *scroll, *offset))
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_owned())
}

fn decode_positions(value: &str) -> Vec<(PathBuf, f32, u64)> {
    serde_json::from_str::<Vec<(String, f32, u64)>>(value)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, scroll, _)| scroll.is_finite())
        .map(|(path, scroll, offset)| (PathBuf::from(path), scroll, offset))
        .collect()
}

pub(crate) fn config_dir() -> Option<PathBuf> {
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

fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings"))
}

fn language_path() -> Option<PathBuf> {
    Some(config_dir()?.join("language"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_prefs() -> Prefs {
        Prefs {
            lang: Lang::English,
            font: String::new(),
            size: 15.0,
            spellcheck: false,
            theme: ThemePref::System,
            line_wrap: true,
            recent: Vec::new(),
            positions: Vec::new(),
        }
    }

    #[test]
    fn recent_files_are_deduplicated_and_most_recent_first() {
        let mut prefs = test_prefs();
        prefs.add_recent(Path::new("first.txt"));
        prefs.add_recent(Path::new("second.txt"));
        prefs.add_recent(Path::new("first.txt"));
        assert_eq!(
            prefs.recent,
            vec![PathBuf::from("first.txt"), PathBuf::from("second.txt")]
        );
    }

    #[test]
    fn recent_and_positions_roundtrip_as_json() {
        let paths = vec![PathBuf::from("a file.txt"), PathBuf::from("æøå.txt")];
        assert_eq!(decode_recent(&encode_recent(&paths)), paths);

        let positions = vec![(PathBuf::from("log.txt"), 123.5, 9876)];
        assert_eq!(decode_positions(&encode_positions(&positions)), positions);
    }

    #[test]
    fn position_updates_without_duplicates() {
        let mut prefs = test_prefs();
        let path = Path::new("note.txt");
        prefs.set_position(path, 10.0, 20);
        prefs.set_position(path, 30.0, 40);
        assert_eq!(prefs.position(path), (30.0, 40));
        assert_eq!(prefs.positions.len(), 1);
    }
}

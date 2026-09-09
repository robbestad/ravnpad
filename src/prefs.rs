use std::path::PathBuf;

use crate::i18n::{self, Lang};

#[derive(Clone, Debug)]
pub struct Prefs {
    pub lang: Lang,
    pub font: String,
    pub size: f32,
}

impl Prefs {
    pub fn load() -> Self {
        let mut prefs = Self {
            lang: i18n::detect(),
            font: String::new(),
            size: 15.0,
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
                        "size" => {
                            if let Ok(size) = value.trim().parse::<f32>() {
                                prefs.size = size.clamp(10.0, 36.0);
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
        prefs
    }

    pub fn save(&self) {
        if let Some(path) = settings_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let body = format!(
                "lang={}\nfont={}\nsize={}\n",
                self.lang.code(),
                self.font,
                self.size
            );
            let _ = std::fs::write(path, body);
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("RavnPad"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/RavnPad"),
        )
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

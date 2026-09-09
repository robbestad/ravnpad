use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

#[derive(Clone)]
pub struct FontChoice {
    pub id: String,
    pub name: String,
    pub path: Option<PathBuf>,
}

pub fn available_fonts() -> Vec<FontChoice> {
    let mut fonts = vec![FontChoice {
        id: String::new(),
        name: String::new(),
        path: None,
    }];
    let mut seen = std::collections::BTreeSet::new();
    for dir in font_dirs() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !ext.eq_ignore_ascii_case("ttf") && !ext.eq_ignore_ascii_case("otf") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let lower = stem.to_ascii_lowercase();
            if is_style_variant(&lower) {
                continue;
            }
            let id = stem.to_owned();
            if !seen.insert(id.to_ascii_lowercase()) {
                continue;
            }
            fonts.push(FontChoice {
                name: pretty_name(stem),
                id,
                path: Some(path),
            });
        }
    }
    fonts.sort_by(|a, b| {
        if a.id.is_empty() {
            std::cmp::Ordering::Less
        } else if b.id.is_empty() {
            std::cmp::Ordering::Greater
        } else {
            a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())
        }
    });
    fonts
}

pub fn apply(ctx: &egui::Context, font_id: &str, size: f32, fonts: &[FontChoice]) {
    let mut defs = FontDefinitions::default();
    if let Some(bytes) = load_ui_font() {
        defs.font_data
            .insert("system_ui".into(), Arc::new(FontData::from_owned(bytes)));
        if let Some(prop) = defs.families.get_mut(&FontFamily::Proportional) {
            prop.insert(0, "system_ui".into());
        }
        if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
            mono.push("system_ui".into());
        }
    }
    if !font_id.is_empty()
        && let Some(choice) = fonts.iter().find(|font| font.id == font_id)
        && let Some(path) = &choice.path
        && let Ok(bytes) = std::fs::read(path)
    {
        defs.font_data
            .insert("editor".into(), Arc::new(FontData::from_owned(bytes)));
        if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
            mono.insert(0, "editor".into());
        }
    }
    ctx.set_fonts(defs);

    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Small, FontId::new(12.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(size.clamp(10.0, 36.0), FontFamily::Monospace),
    );
    ctx.set_style(style);
}

fn is_style_variant(name: &str) -> bool {
    ["bold", "italic", "oblique", "black", "light", "thin", "medium", "heavy"]
        .iter()
        .any(|part| name.contains(part))
}

fn pretty_name(stem: &str) -> String {
    match stem.to_ascii_lowercase().as_str() {
        "consola" => "Consolas".into(),
        "cour" => "Courier New".into(),
        "lucon" => "Lucida Console".into(),
        "segoeui" => "Segoe UI".into(),
        "arial" => "Arial".into(),
        "calibri" => "Calibri".into(),
        "cambria" => "Cambria".into(),
        "candara" => "Candara".into(),
        "comic" => "Comic Sans MS".into(),
        "georgia" => "Georgia".into(),
        "tahoma" => "Tahoma".into(),
        "times" => "Times New Roman".into(),
        "trebuc" => "Trebuchet MS".into(),
        "verdana" => "Verdana".into(),
        "cascadiamono" => "Cascadia Mono".into(),
        "cascadiacode" => "Cascadia Code".into(),
        "dejavusansmono" => "DejaVu Sans Mono".into(),
        "dejavusans" => "DejaVu Sans".into(),
        "liberationmono" => "Liberation Mono".into(),
        "nunitosans" => "Nunito Sans".into(),
        _ => stem.replace('-', " ").replace('_', " "),
    }
}

fn load_ui_font() -> Option<Vec<u8>> {
    for path in ui_font_candidates() {
        if let Ok(bytes) = std::fs::read(&path)
            && !bytes.is_empty()
        {
            return Some(bytes);
        }
    }
    None
}

fn ui_font_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in font_dirs() {
        for name in [
            "segoeui.ttf",
            "SegoeUI.ttf",
            "arial.ttf",
            "Arial.ttf",
            "DejaVuSans.ttf",
            "NotoSans-Regular.ttf",
            "FreeSans.ttf",
            "Ubuntu-R.ttf",
            "Helvetica.ttc",
            "SFNS.ttf",
            "LucidaGrande.ttc",
            "Geneva.ttf",
        ] {
            paths.push(dir.join(name));
        }
    }
    paths
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
        dirs.push(Path::new(&windir).join("Fonts"));
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(Path::new(&home).join("Library/Fonts"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts/truetype"));
        dirs.push(PathBuf::from("/usr/share/fonts/TTF"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(Path::new(&home).join(".fonts"));
            dirs.push(Path::new(&home).join(".local/share/fonts"));
        }
    }
    dirs
}

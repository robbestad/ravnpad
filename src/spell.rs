use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::i18n::Lang;
use crate::prefs;

const BASE: &str = "https://raw.githubusercontent.com/wooorm/dictionaries/main/dictionaries";
const USER_AGENT: &str = concat!(
    "RavnPad/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/robbestad/ravnpad)"
);

#[derive(Debug)]
pub enum SpellError {
    Unavailable,
    Download(String),
    Parse(String),
    Io(std::io::Error),
}

impl std::fmt::Display for SpellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "no dictionary for this language"),
            Self::Download(msg) | Self::Parse(msg) => write!(f, "{msg}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Miss {
    pub cchar: usize, // char index of the word start
    pub byte: usize,  // byte offset of the word start
    pub len: usize,   // word length in chars
}

// Hunspell dictionary directories in wooorm/dictionaries. Finnish is
// missing there (LibreOffice uses Voikko instead), so it gets no list.
fn locales(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::English => &["en"],
        Lang::Bokmal => &["nb"],
        Lang::Nynorsk => &["nn"],
        Lang::Swedish => &["sv"],
        Lang::Danish => &["da"],
        Lang::Icelandic => &["is"],
        Lang::German => &["de"],
        Lang::Dutch => &["nl"],
        Lang::French => &["fr"],
        Lang::Spanish => &["es"],
        Lang::Italian => &["it"],
        Lang::Portuguese => &["pt", "pt-PT"],
        Lang::Finnish => &[],
        Lang::Polish => &["pl"],
        Lang::Czech => &["cs"],
    }
}

// Loads the dictionary for the UI language, downloading it from
// wooorm/dictionaries on first use. Called off the main thread.
pub fn load(lang: Lang) -> Result<spellbook::Dictionary, SpellError> {
    let candidates = locales(lang);
    if candidates.is_empty() {
        return Err(SpellError::Unavailable);
    }
    let dir = prefs::config_dir()
        .map(|dir| dir.join("dicts"))
        .ok_or(SpellError::Unavailable)?;
    let mut last_err = SpellError::Unavailable;
    for locale in candidates {
        match ensure_files(&dir, locale) {
            Ok((aff, dic)) => {
                let mut dict = spellbook::Dictionary::new(&aff, &dic)
                    .map_err(|err| SpellError::Parse(err.to_string()))?;
                apply_personal(&mut dict);
                return Ok(dict);
            }
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

fn ensure_files(dir: &Path, locale: &str) -> Result<(String, String), SpellError> {
    let ldir = dir.join(locale);
    let aff_path = ldir.join("index.aff");
    let dic_path = ldir.join("index.dic");
    if !(aff_path.is_file() && dic_path.is_file()) {
        fs::create_dir_all(&ldir).map_err(SpellError::Io)?;
        for ext in ["aff", "dic"] {
            let dest = ldir.join(format!("index.{ext}"));
            let tmp = ldir.join(format!("index.{ext}.part"));
            fetch(&format!("{BASE}/{locale}/index.{ext}"), &tmp)?;
            fs::rename(&tmp, &dest).map_err(SpellError::Io)?;
        }
    }
    let aff = fs::read_to_string(&aff_path).map_err(SpellError::Io)?;
    let dic = fs::read_to_string(&dic_path).map_err(SpellError::Io)?;
    Ok((aff, dic))
}

fn fetch(url: &str, dest: &Path) -> Result<(), SpellError> {
    let response = ureq::builder()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| SpellError::Download(err.to_string()))?;
    let mut file = fs::File::create(dest).map_err(SpellError::Io)?;
    std::io::copy(&mut response.into_reader(), &mut file).map_err(SpellError::Io)?;
    Ok(())
}

fn personal_path() -> Option<std::path::PathBuf> {
    prefs::config_dir().map(|dir| dir.join("dicts/personal.txt"))
}

// Words added by the user via the context menu, applied on top of every
// downloaded dictionary.
fn apply_personal(dict: &mut spellbook::Dictionary) {
    if let Some(path) = personal_path()
        && let Ok(text) = fs::read_to_string(path)
    {
        for line in text.lines() {
            let _ = dict.add(line.trim());
        }
    }
}

pub fn add_personal(word: &str) -> std::io::Result<()> {
    let Some(path) = personal_path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{word}")
}

// Word spans as (char index, byte index, &str word) for spell checking.
// Apostrophes inside a word are treated as part of it ("don't", "fisk'").
pub fn word_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let mut spans = Vec::new();
    let mut start = None; // (char index, byte index)
    for (cchar, (byte, ch)) in text.char_indices().enumerate() {
        let word_char = ch.is_alphanumeric() || ch == '\'' || ch == '’';
        match (word_char, start) {
            (true, None) => start = Some((cchar, byte)),
            (false, Some((cs, bs))) => {
                spans.push((cs, bs, &text[bs..byte]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some((cs, bs)) = start {
        spans.push((cs, bs, &text[bs..]));
    }
    spans
}

// Returns one Miss per misspelled word.
pub fn misspellings(text: &str, dict: &spellbook::Dictionary) -> Vec<Miss> {
    word_spans(text)
        .into_iter()
        .filter(|(_, _, word)| !skip_word(word) && !dict.check(word))
        .map(|(cchar, byte, word)| Miss {
            cchar,
            byte,
            len: word.chars().count(),
        })
        .collect()
}

fn skip_word(word: &str) -> bool {
    if !word.chars().any(char::is_alphabetic) {
        return true; // numbers and punctuation
    }
    // Acronyms and all-caps words (UTF-8, NASA) are rarely in dictionaries.
    word.chars().count() > 1 && !word.chars().any(char::is_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_spans_splits_on_punctuation() {
        let spans: Vec<&str> = word_spans("hei, verden! don't")
            .into_iter()
            .map(|(_, _, word)| word)
            .collect();
        assert_eq!(spans, vec!["hei", "verden", "don't"]);
    }

    #[test]
    fn word_spans_keeps_numbers() {
        let spans: Vec<&str> = word_spans("abc 123 def")
            .into_iter()
            .map(|(_, _, word)| word)
            .collect();
        assert_eq!(spans, vec!["abc", "123", "def"]);
    }

    // Downloads the real nb dictionary once; cached under the config dir.
    #[test]
    #[ignore]
    fn loads_nb_dictionary() {
        let mut dict = load(Lang::Bokmal).unwrap();
        assert!(dict.check("hund"));
        assert!(!dict.check("hudn"));
        dict.add("hudn").unwrap();
        assert!(dict.check("hudn"));
        let mut suggestions = Vec::new();
        dict.suggest("hnud", &mut suggestions);
        assert!(!suggestions.is_empty());
    }
}

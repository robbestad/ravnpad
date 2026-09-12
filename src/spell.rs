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
// Apostrophes and hyphens inside a word are treated as part of it
// ("don't", "stålrørs-stol") so compounds are checked and replaced whole.
const SEPARATORS: [char; 3] = ['-', '\u{2010}', '\u{2011}'];

pub fn word_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let mut spans = Vec::new();
    let mut start = None; // (char index, byte index)
    for (cchar, (byte, ch)) in text.char_indices().enumerate() {
        let word_char = ch.is_alphanumeric() || ch == '\'' || ch == '’' || SEPARATORS.contains(&ch);
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

// Only consult the dictionary for words overlapping the viewport. Keep full
// words and document-relative offsets so edge words can still be replaced.
pub fn misspellings(
    text: &str,
    dict: &spellbook::Dictionary,
    visible: std::ops::Range<usize>,
) -> Vec<Miss> {
    let mut checked = std::collections::HashMap::new();
    word_spans(text)
        .into_iter()
        .take_while(|(cchar, _, _)| *cchar < visible.end)
        .filter_map(|(cchar, byte, word)| {
            let len = word.chars().count();
            if cchar + len <= visible.start {
                return None;
            }
            let correct = *checked.entry(word).or_insert_with(|| is_correct(word, dict));
            (!correct).then_some(Miss { cchar, byte, len })
        })
        .collect()
}

fn is_correct(word: &str, dict: &spellbook::Dictionary) -> bool {
    if skip_word(word) || dict.check(word) {
        return true;
    }
    // The dictionaries have no BREAK rules, so hyphenated compounds fail
    // as a whole even when every part is a real word — check the parts.
    word.contains(SEPARATORS)
        && part_spans(word)
            .into_iter()
            .all(|(s, e)| is_correct(&word[s..e], dict))
}

// Byte spans of the word's hyphen-separated parts.
fn part_spans(word: &str) -> Vec<(usize, usize)> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, ch) in word.char_indices() {
        if SEPARATORS.contains(&ch) {
            parts.push((start, i));
            start = i + ch.len_utf8();
        }
    }
    parts.push((start, word.len()));
    parts.into_iter().filter(|(s, e)| s < e).collect()
}

// Suggestions for the context menu. For hyphenated words the dictionary
// suggestion engine knows nothing, so also offer the compound rebuilt
// with each misspelled part corrected.
pub fn suggest(word: &str, dict: &spellbook::Dictionary) -> Vec<String> {
    let mut out = Vec::new();
    dict.suggest(word, &mut out);
    if word.contains(SEPARATORS) {
        for (s, e) in part_spans(word) {
            let part = &word[s..e];
            if is_correct(part, dict) {
                continue;
            }
            let mut part_sugg = Vec::new();
            dict.suggest(part, &mut part_sugg);
            for suggestion in part_sugg.into_iter().take(3) {
                out.push(format!("{}{}{}", &word[..s], suggestion, &word[e..]));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.clone()));
    out
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
    fn viewport_checks_keep_unicode_offsets_and_whole_edge_words() {
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let text = "hello æøå-feil hello feil";
        let misses = misspellings(text, &dict, 8..10);
        assert_eq!(misses.len(), 1);
        assert_eq!(misses[0].cchar, 6);
        assert_eq!(misses[0].byte, 6);
        assert_eq!(misses[0].len, 8);
        assert!(misspellings(text, &dict, 15..20).is_empty());
    }

    #[test]
    fn large_document_only_checks_visible_words() {
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let prefix = "æøå unknown\n".repeat(20_000);
        assert!(prefix.len() > 200 * 1024);
        let offset = prefix.chars().count();
        let text = format!("{prefix}hello typo tail");
        let misses = misspellings(&text, &dict, offset..offset + 10);
        assert_eq!(misses.len(), 1);
        assert_eq!(misses[0].cchar, offset + 6);
        assert_eq!(misses[0].byte, prefix.len() + 6);
        assert_eq!(misses[0].len, 4);
    }

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

    #[test]
    fn word_spans_keeps_hyphenated_words_whole() {
        let spans: Vec<&str> = word_spans("en stålrørs-stol og – ja")
            .into_iter()
            .map(|(_, _, word)| word)
            .collect();
        assert_eq!(spans, vec!["en", "stålrørs-stol", "og", "ja"]);
    }

    #[test]
    fn part_spans_splits_on_all_hyphens() {
        assert_eq!(
            part_spans("stål-rørs\u{2010}stol")
                .into_iter()
                .map(|(s, e)| "stål-rørs\u{2010}stol"[s..e].to_owned())
                .collect::<Vec<_>>(),
            vec!["stål", "rørs", "stol"]
        );
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

        // Hyphenated compounds are checked per part.
        assert!(is_correct("hund-hus", &dict));
        assert!(!is_correct("hund-huss", &dict));
        let suggestions = suggest("hund-huss", &dict);
        assert!(suggestions.iter().any(|s| s == "hund-hus"));
    }
}

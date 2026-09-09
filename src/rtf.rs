use std::path::Path;

pub fn path_is_rtf(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rtf"))
}

pub fn looks_like_rtf(bytes: &[u8]) -> bool {
    let start = trim_start(bytes);
    start.starts_with(b"{\\rtf")
}

pub fn to_text(bytes: &[u8]) -> Result<String, ()> {
    if !looks_like_rtf(bytes) {
        return Err(());
    }
    Ok(extract(bytes))
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let mut i = 0;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    &bytes[i..]
}

fn extract(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let mut skip = 0;
    let mut uc = 1i32;
    while i < bytes.len() {
        i = consume(bytes, i, 0, &mut out, &mut skip, &mut uc);
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn consume(
    bytes: &[u8],
    mut i: usize,
    depth: u32,
    out: &mut String,
    skip: &mut i32,
    uc: &mut i32,
) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'}' => {
                if depth > 0 {
                    return i + 1;
                }
                i += 1;
            }
            b'{' => {
                i += 1;
                if is_skip_group(bytes, i) {
                    i = skip_group(bytes, i);
                } else {
                    i = consume(bytes, i, depth + 1, out, skip, uc);
                }
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                i = control(bytes, i, out, skip, uc);
            }
            b'\r' | b'\n' => i += 1,
            byte => {
                if *skip > 0 {
                    *skip -= 1;
                } else if byte >= 0x20 {
                    out.push(byte as char);
                }
                i += 1;
            }
        }
    }
    i
}

fn is_skip_group(bytes: &[u8], i: usize) -> bool {
    let rest = &bytes[i..];
    rest.starts_with(b"\\*")
        || rest.starts_with(b"\\fonttbl")
        || rest.starts_with(b"\\colortbl")
        || rest.starts_with(b"\\stylesheet")
        || rest.starts_with(b"\\info")
        || rest.starts_with(b"\\pict")
        || rest.starts_with(b"\\object")
        || rest.starts_with(b"\\header")
        || rest.starts_with(b"\\footer")
        || rest.starts_with(b"\\footnote")
        || rest.starts_with(b"\\field")
        || rest.starts_with(b"\\shp")
        || rest.starts_with(b"\\themedata")
        || rest.starts_with(b"\\colorschememapping")
        || rest.starts_with(b"\\latentstyles")
        || rest.starts_with(b"\\datastore")
}

fn skip_group(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 1;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'\\' => {
                i += 1;
                if i < bytes.len() && (bytes[i] == b'\\' || bytes[i] == b'{' || bytes[i] == b'}') {
                    i += 1;
                    continue;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn control(bytes: &[u8], mut i: usize, out: &mut String, skip: &mut i32, uc: &mut i32) -> usize {
    let next = bytes[i];
    if next.is_ascii_alphabetic() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
        let mut neg = false;
        if i < bytes.len() && bytes[i] == b'-' {
            neg = true;
            i += 1;
        }
        let num_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let arg = if i > num_start {
            let n = std::str::from_utf8(&bytes[num_start..i])
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            Some(if neg { -n } else { n })
        } else {
            None
        };
        if i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        apply_word(word, arg, out, skip, uc);
        return skip_rtf_newline(bytes, i);
    }

    match next {
        b'\'' => {
            i += 1;
            if i + 1 < bytes.len() {
                if let (Some(hi), Some(lo)) = (from_hex(bytes[i]), from_hex(bytes[i + 1])) {
                    if *skip > 0 {
                        *skip -= 1;
                    } else {
                        push_cp1252(out, (hi << 4) | lo);
                    }
                    i += 2;
                    return i;
                }
            }
        }
        b'~' => {
            if *skip > 0 {
                *skip -= 1;
            } else {
                out.push('\u{00A0}');
            }
        }
        b'_' | b'-' => {
            if *skip > 0 {
                *skip -= 1;
            } else {
                out.push('-');
            }
        }
        b'\\' | b'{' | b'}' => {
            if *skip > 0 {
                *skip -= 1;
            } else {
                out.push(next as char);
            }
        }
        b'\r' | b'\n' => {
            // Cocoa/TextEdit uses backslash + newline as a hard line break.
            if *skip > 0 {
                *skip -= 1;
            } else {
                push_newline(out);
            }
            return skip_rtf_newline(bytes, i);
        }
        _ => {}
    }
    i + 1
}

fn skip_rtf_newline(bytes: &[u8], mut i: usize) -> usize {
    if i < bytes.len() && bytes[i] == b'\r' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'\n' {
        i += 1;
    }
    i
}

fn push_newline(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

fn apply_word(word: &str, arg: Option<i32>, out: &mut String, skip: &mut i32, uc: &mut i32) {
    match word {
        "par" | "line" | "page" | "sect" | "row" => push_newline(out),
        "tab" | "cell" => out.push('\t'),
        "uc" => {
            if let Some(n) = arg {
                *uc = n.max(0);
            }
        }
        "u" => {
            if let Some(n) = arg {
                let code = if n < 0 { (n as i32 as u16) as u32 } else { n as u32 };
                if let Some(ch) = char::from_u32(code) {
                    out.push(ch);
                }
                *skip = *uc;
            }
        }
        "bin" => {}
        "emdash" => out.push('—'),
        "endash" => out.push('–'),
        "lquote" => out.push('\u{2018}'),
        "rquote" => out.push('\u{2019}'),
        "ldblquote" => out.push('\u{201C}'),
        "rdblquote" => out.push('\u{201D}'),
        "bullet" => out.push('•'),
        _ => {}
    }
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_cp1252(out: &mut String, byte: u8) {
    const MAP: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '•', '–', '—', '˜', '™',
        'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    if byte < 0x80 {
        out.push(byte as char);
    } else if byte < 0xA0 {
        out.push(MAP[(byte - 0x80) as usize]);
    } else {
        out.push(char::from_u32(byte as u32).unwrap_or('\u{FFFD}'));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_and_bold() {
        let rtf = br#"{\rtf1\ansi{\fonttbl\f0 Arial;}{\colortbl ;}\pard\f0 Hei {\b verden}\par}"#;
        assert_eq!(to_text(rtf).unwrap(), "Hei verden");
    }

    #[test]
    fn keeps_par_newlines() {
        let rtf = b"{\\rtf1\\ansi\\pard line one\\par\r\nline two\\par\r\n}";
        assert_eq!(to_text(rtf).unwrap(), "line one\nline two");
    }

    #[test]
    fn keeps_backslash_newlines() {
        let rtf = b"{\\rtf1\\ansi\\pard line one\\\r\nline two\\\r\n}";
        assert_eq!(to_text(rtf).unwrap(), "line one\nline two");
    }

    #[test]
    fn decodes_windows_1252() {
        let rtf = br#"{\rtf1\ansi\ansicpg1252\pard bl\'e5b\'e6r\'f8re\par}"#;
        assert_eq!(to_text(rtf).unwrap(), "blåbærøre");
    }

    #[test]
    fn decodes_unicode() {
        let rtf = br#"{\rtf1\ansi\pard caf\u233\'3f\par}"#;
        assert_eq!(to_text(rtf).unwrap(), "café");
    }

    #[test]
    fn rejects_non_rtf() {
        assert!(to_text(b"hello").is_err());
    }

    #[test]
    fn path_extension() {
        assert!(path_is_rtf(Path::new("note.RTF")));
        assert!(!path_is_rtf(Path::new("note.txt")));
    }
}

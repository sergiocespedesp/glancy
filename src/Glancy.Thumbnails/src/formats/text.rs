//! Text documents (text, XML, RTF, email, HTML and MHTML): the first lines are extracted and drawn on a page (see
//! `text_page`).
//!
//! - Plain text: UTF-8 (with or without BOM), UTF-16 with BOM or, failing that, Windows-1252; .nfo/.diz in code
//!   page 437 (box-drawing art).
//! - RTF: control groups are discarded and `\'hh` (according to `\ansicpg`) and `\uN` are decoded.
//! - Email (.eml) and MHTML: MIME traversal with quoted-printable/base64 and character sets; encoded headers
//!   (RFC 2047) are decoded.
//! - HTML: `<script>`/`<style>` and tags removed, entities decoded; `<title>` becomes the page title.

use crate::error::Result;
use crate::formats::text_page::{self, Doc};
use crate::formats::{Content, Ctx};

/// Maximum amount read from the file.
const MAX_READ: usize = 256 * 1024;
const MAX_LINES: usize = 80;
const MAX_CHARS: usize = 6 * 1024;
const MAX_MIME_DEPTH: u32 = 6;

// ---------------------------------------------------------------------------------------------------------------
// Character sets

/// Windows-1252 from 0x80 to 0x9F (the rest matches Latin-1).
const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’',
    '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

/// Code page 437 from 0x80 to 0xFF.
const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{A0}";

fn cp1252(bytes: &[u8]) -> String {
    bytes.iter().map(|b| if (0x80..0xA0).contains(b) { CP1252[(b - 0x80) as usize] } else { *b as char }).collect()
}

fn cp437(bytes: &[u8]) -> String {
    let high: Vec<char> = CP437.chars().collect();
    bytes.iter().map(|b| if *b >= 0x80 { high.get((b - 0x80) as usize).copied().unwrap_or('?') } else { *b as char }).collect()
}

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|c| if big_endian { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
    char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')).collect()
}

/// Bytes in a Windows code page.
fn decode_cp(bytes: &[u8], cp: u32) -> String {
    match cp {
        65001 => String::from_utf8_lossy(bytes).into_owned(),
        1252 | 28591 | 20127 => cp1252(bytes),
        437 => cp437(bytes),
        1200 => utf16(bytes, false),
        1201 => utf16(bytes, true),
        _ => {
            use windows::Win32::Globalization::{MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
            if bytes.is_empty() {
                return String::new();
            }
            let mut out = vec![0u16; bytes.len()];
            // In Windows code pages, a byte never yields more than one UTF-16 unit.
            let n = unsafe { MultiByteToWideChar(cp, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, Some(&mut out)) };
            if n <= 0 { cp1252(bytes) } else { String::from_utf16_lossy(&out[..n as usize]) }
        }
    }
}

/// Code page for a MIME/HTML character set name.
fn charset_cp(name: &str) -> Option<u32> {
    let n = name.trim().trim_matches('"').to_ascii_lowercase();
    let cp = match n.as_str() {
        "utf-8" | "utf8" => 65001,
        "us-ascii" | "ascii" | "iso-8859-1" | "latin1" | "iso_8859-1" | "windows-1252" | "cp1252" => 1252,
        "utf-16" | "utf-16le" => 1200,
        "utf-16be" => 1201,
        "koi8-r" => 20866,
        "koi8-u" => 21866,
        "shift_jis" | "shift-jis" | "sjis" | "x-sjis" | "windows-31j" => 932,
        "euc-jp" => 20932,
        "iso-2022-jp" => 50220,
        "gb2312" | "gbk" | "x-gbk" => 936,
        "gb18030" => 54936,
        "big5" => 950,
        "euc-kr" | "ks_c_5601-1987" => 949,
        "ibm437" | "cp437" => 437,
        "macintosh" => 10000,
        _ => {
            if let Some(v) = n.strip_prefix("windows-").or_else(|| n.strip_prefix("cp")) {
                return v.parse().ok().filter(|v| (874..=1258).contains(v));
            }
            let v: u32 = n.strip_prefix("iso-8859-").or_else(|| n.strip_prefix("iso8859-"))?.parse().ok()?;
            return (1..=16).contains(&v).then_some(28590 + v);
        }
    };
    Some(cp)
}

/// Text with no encoding declared: BOM, valid UTF-8 or Windows-1252 (`fallback`).
fn sniff_decode(bytes: &[u8], fallback: u32) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => return s.to_string(),
        // Read cut off in the middle of a character: still UTF-8.
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => return String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
        Err(_) => {}
    }
    // UTF-16 LE without BOM: many zeros at odd positions.
    let sample = &bytes[..bytes.len().min(512)];
    let zeros = sample.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    if sample.len() >= 8 && zeros * 4 > sample.len() {
        return utf16(bytes, false);
    }
    decode_cp(bytes, fallback)
}

/// Normalized line breaks, no control characters (except tab) and trimmed to the limits.
fn tidy(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i >= MAX_LINES || out.len() >= MAX_CHARS {
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out.extend(line.chars().filter(|c| !c.is_control() || *c == '\t').take(MAX_CHARS));
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// RTF

/// Destinations whose content is not document text.
const RTF_SKIP: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "headerl", "headerr", "headerf", "footer", "footerl",
    "footerr", "footerf", "listtable", "listoverridetable", "rsidtbl", "generator", "xmlnstbl", "themedata",
    "colorschememapping", "datastore", "latentstyles", "object", "fldinst", "filetbl", "revtbl", "pgdsctbl", "mmathPr",
];

fn rtf_to_text(data: &[u8]) -> String {
    struct State {
        skip: bool,
        uc: usize,
    }
    let mut stack = vec![State { skip: false, uc: 1 }];
    let mut out = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut cp = 1252u32;
    let mut skip_chars = 0usize;
    let mut i = 0usize;
    let flush = |pending: &mut Vec<u8>, out: &mut String, cp: u32| {
        if !pending.is_empty() {
            out.push_str(&decode_cp(pending, cp));
            pending.clear();
        }
    };
    while i < data.len() && out.len() < MAX_CHARS * 2 {
        let c = data[i];
        let skipping = stack.last().is_some_and(|s| s.skip);
        match c {
            b'{' => {
                flush(&mut pending, &mut out, cp);
                let skip = skipping || data.get(i + 1..i + 3) == Some(b"\\*");
                let uc = stack.last().map_or(1, |s| s.uc);
                stack.push(State { skip, uc });
                i += 1;
            }
            b'}' => {
                flush(&mut pending, &mut out, cp);
                if stack.len() > 1 {
                    stack.pop();
                }
                i += 1;
            }
            b'\\' => {
                let Some(&n) = data.get(i + 1) else { break };
                if n == b'\'' {
                    // \'hh: one byte in the document's code page.
                    let hex = data.get(i + 2..i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                    i += 4;
                    if skip_chars > 0 {
                        skip_chars -= 1;
                    } else if let (Some(b), false) = (hex, skipping) {
                        pending.push(b);
                    }
                    continue;
                }
                flush(&mut pending, &mut out, cp);
                if !n.is_ascii_alphabetic() {
                    // Control symbol.
                    i += 2;
                    if skip_chars > 0 {
                        skip_chars -= 1;
                        continue;
                    }
                    if !skipping {
                        match n {
                            b'\\' | b'{' | b'}' => out.push(n as char),
                            b'~' => out.push('\u{A0}'),
                            b'_' => out.push('-'),
                            b'\n' | b'\r' => out.push('\n'),
                            _ => {}
                        }
                    }
                    continue;
                }
                let start = i + 1;
                let mut j = start;
                while j < data.len() && data[j].is_ascii_alphabetic() && j - start < 32 {
                    j += 1;
                }
                let word = std::str::from_utf8(&data[start..j]).unwrap_or("");
                let num_start = j;
                if j < data.len() && data[j] == b'-' {
                    j += 1;
                }
                while j < data.len() && data[j].is_ascii_digit() && j - num_start < 10 {
                    j += 1;
                }
                let param: Option<i64> = std::str::from_utf8(&data[num_start..j]).ok().and_then(|s| s.parse().ok());
                if j < data.len() && data[j] == b' ' {
                    j += 1;
                }
                i = j;
                if word == "bin" {
                    i = i.saturating_add(param.unwrap_or(0).max(0) as usize);
                    continue;
                }
                if skip_chars > 0 {
                    skip_chars -= 1;
                    continue;
                }
                if RTF_SKIP.contains(&word) {
                    if let Some(s) = stack.last_mut() {
                        s.skip = true;
                    }
                    continue;
                }
                match word {
                    "ansicpg" => cp = param.and_then(|p| u32::try_from(p).ok()).unwrap_or(1252),
                    "uc" => {
                        if let Some(s) = stack.last_mut() {
                            s.uc = param.unwrap_or(1).clamp(0, 8) as usize;
                        }
                    }
                    _ if skipping => {}
                    "u" => {
                        let v = param.unwrap_or(0);
                        let v = if v < 0 { v + 65_536 } else { v };
                        out.push(char::from_u32(v as u32).unwrap_or('\u{FFFD}'));
                        skip_chars = stack.last().map_or(1, |s| s.uc);
                    }
                    "par" | "line" | "row" | "sect" | "page" => out.push('\n'),
                    "tab" | "cell" => out.push('\t'),
                    "emdash" => out.push('—'),
                    "endash" => out.push('–'),
                    "bullet" => out.push('•'),
                    "lquote" => out.push('‘'),
                    "rquote" => out.push('’'),
                    "ldblquote" => out.push('“'),
                    "rdblquote" => out.push('”'),
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                i += 1;
                if skip_chars > 0 {
                    skip_chars -= 1;
                } else if !skipping {
                    pending.push(c);
                }
            }
        }
    }
    flush(&mut pending, &mut out, cp);
    out
}

// ---------------------------------------------------------------------------------------------------------------
// MIME (email and MHTML)

fn base64(data: &[u8]) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for v in data.iter().take_while(|c| **c != b'=').filter_map(|c| val(*c)) {
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Quoted-printable. `underscore`: in encoded words in headers, "_" is a space.
fn quoted_printable(data: &[u8], underscore: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'=' => {
                let hex = data.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(b) = hex {
                    out.push(b);
                    i += 3;
                } else if data.get(i + 1) == Some(&b'\r') && data.get(i + 2) == Some(&b'\n') {
                    i += 3;
                } else if data.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            b'_' if underscore => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Header with encoded words `=?charset?Q|B?text?=` (RFC 2047).
fn decode_header(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut last_encoded = false;
    while let Some(start) = rest.find("=?") {
        let (before, after) = rest.split_at(start);
        let parsed = (|| {
            let body = &after[2..];
            let q1 = body.find('?')?;
            let enc = body.get(q1 + 1..q1 + 2)?;
            if body.get(q1 + 2..q1 + 3)? != "?" {
                return None;
            }
            let text_start = q1 + 3;
            let end = body[text_start..].find("?=")? + text_start;
            let charset = body[..q1].split('*').next().unwrap_or("");
            let raw = &body.as_bytes()[text_start..end];
            let bytes = match enc {
                "B" | "b" => base64(raw),
                "Q" | "q" => quoted_printable(raw, true),
                _ => return None,
            };
            Some((decode_cp(&bytes, charset_cp(charset).unwrap_or(1252)), end + 4))
        })();
        match parsed {
            Some((text, used)) => {
                // Whitespace between two encoded words is ignored.
                if !(last_encoded && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &after[used..];
                last_encoded = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &after[2..];
                last_encoded = false;
            }
        }
    }
    out.push_str(rest);
    out
}

struct Part<'a> {
    headers: Vec<(String, String)>,
    body: &'a [u8],
}

impl Part<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Lowercase MIME type and its parameters.
    fn content_type(&self) -> (String, Option<String>, Option<String>) {
        let v = self.header("content-type").unwrap_or("text/plain");
        let mut it = v.split(';');
        let mime = it.next().unwrap_or("").trim().to_ascii_lowercase();
        let (mut charset, mut boundary) = (None, None);
        for p in it {
            if let Some((k, val)) = p.split_once('=') {
                let val = val.trim().trim_matches('"').to_string();
                match k.trim().to_ascii_lowercase().as_str() {
                    "charset" => charset = Some(val),
                    "boundary" => boundary = Some(val),
                    _ => {}
                }
            }
        }
        (mime, charset, boundary)
    }

    /// Decoded body (transfer encoding and character set).
    fn text(&self, default_cp: u32) -> String {
        let enc = self.header("content-transfer-encoding").unwrap_or("").trim().to_ascii_lowercase();
        let bytes = match enc.as_str() {
            "base64" => base64(&self.body.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect::<Vec<_>>()),
            "quoted-printable" => quoted_printable(self.body, false),
            _ => self.body.to_vec(),
        };
        match self.content_type().1.and_then(|c| charset_cp(&c)) {
            Some(cp) => decode_cp(&bytes, cp),
            None => sniff_decode(&bytes, default_cp),
        }
    }
}

fn parse_part(data: &[u8]) -> Part<'_> {
    let (head, body) = match find_bytes(data, b"\r\n\r\n") {
        Some(p) => (&data[..p], &data[p + 4..]),
        None => match find_bytes(data, b"\n\n") {
            Some(p) => (&data[..p], &data[p + 2..]),
            None => (data, &data[data.len()..]),
        },
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(head).replace("\r\n", "\n").split('\n') {
        if line.starts_with([' ', '\t']) {
            // Folded header: continues the previous one.
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Part { headers, body }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// First part of type `want` (e.g. "text/plain") that is not an attachment, walking through multiparts.
fn find_part<'a>(data: &'a [u8], want: &str, depth: u32) -> Option<Part<'a>> {
    let part = parse_part(data);
    let (mime, _, boundary) = part.content_type();
    if mime.starts_with("multipart/") {
        if depth >= MAX_MIME_DEPTH {
            return None;
        }
        let delim = format!("--{}", boundary?);
        let mut chunks = Vec::new();
        let mut rest = part.body;
        let mut closed = false;
        while let Some(p) = find_bytes(rest, delim.as_bytes()) {
            chunks.push(&rest[..p]);
            rest = &rest[p + delim.len()..];
            if rest.starts_with(b"--") {
                closed = true;
                break;
            }
        }
        // No closing delimiter (truncated message): the rest is the last part; with one, it is the epilogue.
        if !closed {
            chunks.push(rest);
        }
        // The first chunk is the preamble.
        return chunks.into_iter().skip(1).find_map(|c| {
            let c = c.strip_prefix(b"\r\n").or_else(|| c.strip_prefix(b"\n")).unwrap_or(c);
            find_part(c, want, depth + 1)
        });
    }
    let attachment = part.header("content-disposition").is_some_and(|d| d.trim().to_ascii_lowercase().starts_with("attachment"));
    (mime == want && !attachment).then_some(part)
}

fn eml(data: &[u8]) -> Doc {
    let top = parse_part(data);
    let header = |n: &str| top.header(n).map(decode_header);
    let mut meta = Vec::new();
    for name in ["From", "To", "Date"] {
        if let Some(v) = header(name) {
            meta.push(format!("{name}: {v}"));
        }
    }
    let body = match find_part(data, "text/plain", 0) {
        Some(p) => p.text(1252),
        None => find_part(data, "text/html", 0).map(|p| html_to_text(&p.text(1252)).1).unwrap_or_default(),
    };
    Doc { title: header("Subject"), meta, body: tidy(&body), mono: false }
}

fn mhtml(data: &[u8]) -> Doc {
    let subject = parse_part(data).header("Subject").map(decode_header);
    if let Some(p) = find_part(data, "text/html", 0) {
        let (title, body) = html_to_text(&p.text(65001));
        return Doc { title: title.or(subject), meta: Vec::new(), body: tidy(&body), mono: false };
    }
    let body = find_part(data, "text/plain", 0).map(|p| p.text(1252)).unwrap_or_default();
    Doc { title: subject, meta: Vec::new(), body: tidy(&body), mono: false }
}

// ---------------------------------------------------------------------------------------------------------------
// HTML

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let v = match num.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(v);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "middot" => '·',
        "bull" => '•',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "iexcl" => '¡',
        "iquest" => '¿',
        "ordf" => 'ª',
        "ordm" => 'º',
        "szlig" => 'ß',
        _ => return accented(name),
    })
}

/// Accented-letter entities: `aacute`, `Ntilde`, `uuml`…
fn accented(name: &str) -> Option<char> {
    const TABLE: &[(&str, &str, &str)] = &[
        ("acute", "aeiouyAEIOUY", "áéíóúýÁÉÍÓÚÝ"),
        ("grave", "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        ("circ", "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        ("uml", "aeiouyAEIOU", "äëïöüÿÄËÏÖÜ"),
        ("tilde", "anoANO", "ãñõÃÑÕ"),
        ("cedil", "cC", "çÇ"),
        ("ring", "aA", "åÅ"),
        ("slash", "oO", "øØ"),
    ];
    let mut chars = name.chars();
    let letter = chars.next()?;
    let suffix = chars.as_str();
    if suffix == "lig" {
        return match letter {
            'a' => Some('æ'),
            'A' => Some('Æ'),
            'o' => Some('œ'),
            'O' => Some('Œ'),
            _ => None,
        };
    }
    let (_, from, to) = TABLE.iter().find(|(s, _, _)| *s == suffix)?;
    let i = from.chars().position(|c| c == letter)?;
    to.chars().nth(i)
}

const BLOCK_TAGS: &[&str] = &[
    "p", "div", "br", "li", "ul", "ol", "tr", "table", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "header",
    "footer", "blockquote", "pre", "hr", "dt", "dd", "dl", "form", "nav", "aside", "main", "figure", "figcaption",
];
const SKIP_TAGS: &[&str] = &["script", "style", "noscript", "template", "svg", "object", "iframe", "head", "title"];

/// (title, text) of an HTML document.
fn html_to_text(html: &str) -> (Option<String>, String) {
    let lower = html.to_ascii_lowercase();
    // First `<title>` in the document (usually inside `<head>`, which is skipped entirely).
    let title = lower.find("<title").and_then(|t| {
        let start = t + lower[t..].find('>')? + 1;
        let stop = lower[start..].find("</title").map_or(html.len(), |p| start + p);
        Some(collapse(&decode_entities(&html[start..stop])))
    });
    let title = title.filter(|t| !t.is_empty());
    let mut out = String::new();
    let mut i = 0usize;
    let mut pre = 0u32;
    let mut space = false;
    let bytes = html.as_bytes();
    while i < bytes.len() && out.len() < MAX_CHARS * 2 {
        let c = bytes[i];
        if c == b'<' {
            if lower[i..].starts_with("<!--") {
                i = lower[i..].find("-->").map_or(bytes.len(), |p| i + p + 3);
                continue;
            }
            let end = match html[i..].find('>') {
                Some(p) => i + p,
                None => break,
            };
            let tag = &lower[i + 1..end];
            let closing = tag.starts_with('/');
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            i = end + 1;
            if !closing && SKIP_TAGS.contains(&name.as_str()) && !tag.ends_with('/') {
                let close = format!("</{name}");
                let stop = lower[i..].find(&close).map_or(bytes.len(), |p| i + p);
                i = lower[stop..].find('>').map_or(bytes.len(), |p| stop + p + 1);
                continue;
            }
            if name == "pre" {
                pre = if closing { pre.saturating_sub(1) } else { pre + 1 };
            }
            if BLOCK_TAGS.contains(&name.as_str()) {
                while out.ends_with(' ') {
                    out.pop();
                }
                // Each <br> is a line break; other blocks only start a new line.
                if name == "br" || !(out.is_empty() || out.ends_with('\n')) {
                    out.push('\n');
                }
                if name == "li" && !closing {
                    out.push_str("• ");
                }
                space = false;
            } else if matches!(name.as_str(), "td" | "th") && !closing {
                out.push_str("  ");
            }
            continue;
        }
        if c == b'&'
            && let Some(semi) = html[i..].find(';').filter(|p| *p <= 10)
            && let Some(ch) = entity(&html[i + 1..i + semi])
        {
            out.push(ch);
            space = false;
            i += semi + 1;
            continue;
        }
        let ch = html[i..].chars().next().unwrap_or(' ');
        i += ch.len_utf8();
        if pre == 0 && ch.is_whitespace() {
            if !space && !out.ends_with('\n') && !out.is_empty() {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(ch);
            space = false;
        }
    }
    // No more than one blank line in a row.
    let mut body = String::new();
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
        blank = if line.trim().is_empty() { blank + 1 } else { 0 };
        if blank <= 1 && !(body.is_empty() && blank > 0) {
            body.push_str(line);
            body.push('\n');
        }
    }
    (title, body)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        match rest.find(';').filter(|e| *e <= 10).and_then(|e| entity(&rest[1..e]).map(|c| (c, e))) {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Character set declared in `<meta charset>` or in `content="…; charset=…"`.
fn html_charset(head: &[u8]) -> Option<u32> {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]).to_ascii_lowercase();
    let p = s.find("charset=")? + 8;
    let v: String = s[p..].trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    charset_cp(&v)
}

fn html(data: &[u8]) -> Doc {
    let has_bom = data.starts_with(&[0xEF, 0xBB, 0xBF]) || data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]);
    let text = match html_charset(data) {
        Some(cp) if !has_bom => decode_cp(data, cp),
        _ => sniff_decode(data, 1252),
    };
    let (title, body) = html_to_text(&text);
    Doc { title, meta: Vec::new(), body: tidy(&body), mono: false }
}

// ---------------------------------------------------------------------------------------------------------------

/// Extracts the document according to the extension.
fn extract(data: &[u8], extension: &str) -> Doc {
    let plain = |fallback: u32, mono: bool| Doc { title: None, meta: Vec::new(), body: tidy(&sniff_decode(data, fallback)), mono };
    match extension {
        ".rtf" => Doc { title: None, meta: Vec::new(), body: tidy(rtf_to_text(data).trim_start()), mono: false },
        ".eml" => eml(data),
        ".htm" | ".html" | ".xhtml" | ".aspx" => html(data),
        ".mht" | ".mhtm" | ".mhtml" => mhtml(data),
        ".nfo" | ".diz" => plain(437, true),
        ".json" | ".xml" | ".reg" | ".log" | ".scp" => plain(1252, true),
        _ => plain(1252, false),
    }
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let data = ctx.src.read_vec(0, MAX_READ)?;
    let doc = extract(&data, ctx.extension);
    Ok(Content::vector(text_page::render(&doc, ctx.size)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    #[test]
    fn plain_encodings() {
        let mut reg = vec![0xFF, 0xFE];
        for u in "Windows Registry Editor Version 5.00\r\n[HKEY_CURRENT_USER\\Software]".encode_utf16() {
            reg.extend_from_slice(&u.to_le_bytes());
        }
        let d = extract(&reg, ".reg");
        assert!(d.mono);
        assert_eq!(d.body, "Windows Registry Editor Version 5.00\n[HKEY_CURRENT_USER\\Software]");
        assert_eq!(extract(b"caf\xE9 \x80", ".txt").body, "café €");
        assert_eq!(extract("año ✓".as_bytes(), ".md").body, "año ✓");
        assert_eq!(extract(b"\xEF\xBB\xBF{\"a\": 1}", ".json").body, "{\"a\": 1}");
        assert_eq!(extract(b"\xC9\xCD\xBB\r\n\xBA\xB0\xBA", ".nfo").body, "╔═╗\n║░║");
        // UTF-16 LE without BOM.
        let raw: Vec<u8> = "hola mundo".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(extract(&raw, ".txt").body, "hola mundo");
    }

    #[test]
    fn line_limits() {
        let long: String = (0..500).map(|i| format!("línea {i}\n")).collect();
        let d = extract(long.as_bytes(), ".log");
        assert_eq!(d.body.lines().count(), MAX_LINES);
    }

    #[test]
    fn rtf() {
        let src = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Riched20;}\pard\f0 Caf\'e9 cuesta 5\u8364? \par{\b Negrita}\tab fin\par}";
        let d = extract(src, ".rtf");
        assert_eq!(d.body, "Café cuesta 5€ \nNegrita\tfin\n");
        // Cyrillic with \ansicpg1251.
        assert_eq!(rtf_to_text(br"{\rtf1\ansicpg1251 \'cf\'f0\'e8}"), "При");
    }

    #[test]
    fn email_multipart() {
        let eml = b"From: =?UTF-8?Q?Jos=C3=A9_P=C3=A9rez?= <jose@example.com>\r\n\
Subject: =?ISO-8859-1?B?Q2Fm6Q==?= y =?UTF-8?Q?m=C3=A1s?=\r\n\
Date: Tue, 14 Oct 2026 10:00:00 +0200\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative;\r\n boundary=\"XYZ\"\r\n\r\n\
preamble\r\n--XYZ\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
Hola, ma=C3=B1ana =\r\nnos vemos.\r\n--XYZ\r\nContent-Type: text/html\r\n\r\n<p>HTML</p>\r\n--XYZ--\r\n";
        let d = extract(eml, ".eml");
        assert_eq!(d.title.as_deref(), Some("Café y más"));
        assert_eq!(d.meta[0], "From: José Pérez <jose@example.com>");
        assert!(d.meta[1].starts_with("Date: Tue"));
        assert_eq!(d.body.trim(), "Hola, mañana nos vemos.");
        // HTML only, in base64.
        let html_only = b"Subject: x\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nPGI+SG9sYTwvYj4gbXVuZG8=\r\n";
        assert_eq!(extract(html_only, ".eml").body.trim(), "Hola mundo");
    }

    #[test]
    fn html_and_mhtml() {
        let page = b"<html><head><title>Mi &amp; p\xC3\xA1gina</title><style>p{color:red}</style>\
<script>alert('x<y')</script></head><body><h1>Hola</h1><p>Uno&nbsp;&lt;dos&gt;   tres &eacute;&#233;&#xE9;</p>\
<!-- comentario --><ul><li>a</li><li>b</li></ul></body></html>";
        let d = extract(page, ".html");
        assert_eq!(d.title.as_deref(), Some("Mi & página"));
        assert_eq!(d.body, "Hola\nUno <dos> tres ééé\n• a\n• b\n");
        let mht = b"MIME-Version: 1.0\r\nSubject: Guardada\r\nContent-Type: multipart/related; boundary=\"B\"\r\n\r\n--B\r\n\
Content-Type: text/html; charset=\"windows-1252\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
<p>Precio: 5 =80</p>\r\n--B--\r\n";
        let d = extract(mht, ".mht");
        assert_eq!(d.title.as_deref(), Some("Guardada"));
        assert_eq!(d.body.trim(), "Precio: 5 €");
    }

    #[test]
    fn charsets() {
        assert_eq!(charset_cp("ISO-8859-2"), Some(28592));
        assert_eq!(charset_cp("windows-1251"), Some(1251));
        assert_eq!(charset_cp("cp-nada"), None);
        assert_eq!(decode_cp(&[0xC0, 0xC1], 1251), "АБ");
        assert_eq!(base64(b"SGVsbG8="), b"Hello");
    }

    #[test]
    fn renders_page() {
        let src = MemorySource(b"Hola mundo\nSegunda linea con texto bastante largo para ocupar".to_vec());
        let f = FormatSettings::defaults(FormatId::get("text").unwrap());
        let c = render(&Ctx { src: &src, path: None, extension: ".txt", size: 256, settings: &f }).unwrap();
        assert!(c.vector);
        assert_eq!((c.image.width, c.image.height), (256, 256));
        // Outside the page: transparent; inner corner: white; there is dark text at the top left.
        assert_eq!(c.image.pixel(5, 128)[3], 0);
        assert_eq!(c.image.pixel(128, 240), [255, 255, 255, 255]);
        let dark = (0..60).flat_map(|y| (40..200).map(move |x| (x, y))).filter(|(x, y)| c.image.pixel(*x, *y)[0] < 160).count();
        assert!(dark > 20, "{dark}");
    }

    #[test]
    fn garbage_does_not_panic() {
        for ext in [".rtf", ".eml", ".html", ".mht", ".nfo", ".txt", ".reg"] {
            for data in [&b""[..], b"\\", b"{\\'", b"=?utf-8?B?", b"<a href=\"&", b"\xFF\xFE\x00", b"Content-Type: multipart/x; boundary=\"\"\r\n\r\n--"] {
                let _ = extract(data, ext);
            }
        }
        assert_eq!(rtf_to_text(br"{\bin99999999999 x}"), "");
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let bases: [(&[u8], &str); 4] = [
            (br"{\rtf1\ansicpg1251{\fonttbl x}\'cf\u-3913?\par{\*\x y}\bin3 abc}", ".rtf"),
            (b"Subject: =?utf-8?B?SG9sYQ==?=\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\na=C3=B1\r\n--b--", ".eml"),
            (b"<html><title>T&amp;</title><script>x</script><p>a&#x41;<br>b</p><!-- c -->", ".html"),
            (b"\xC9\xCD\xBB\r\n\xBA hola \xBA", ".nfo"),
        ];
        for (base, ext) in bases {
            for _ in 0..300 {
                let mut v = base.to_vec();
                for _ in 0..1 + next() % 6 {
                    let i = next() as usize % v.len();
                    v[i] = (next() >> 24) as u8;
                }
                if next() % 4 == 0 {
                    let cut = next() as usize % v.len();
                    v.truncate(cut);
                }
                let _ = extract(&v, ext);
            }
        }
    }
}

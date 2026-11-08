//! The caller's name, from the user's contacts: the vCard files in
//! `$XDG_DATA_HOME/contacts` and its subfolders one level down, as the
//! contacts apps and vdirsyncer keep them. Only names and numbers are read.

use std::path::{Path, PathBuf};

/// Numbers match on their last 9 digits...
const MATCH_DIGITS: usize = 9;
/// ...or whole when either has fewer than 7, as dmail files conversations.
const MIN_DIGITS: usize = 7;

/// The name of the first contact with `number`.
pub fn name_of(number: &str) -> Option<String> {
    let folder = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?
        .join("contacts");
    cards(&folder).into_iter().find_map(|path| {
        let text = std::fs::read(path).ok()?;
        find(&String::from_utf8_lossy(&text), number)
    })
}

/// The vCard files in `folder` and one level down, hidden ones left out,
/// sorted so the same contact wins every time.
fn cards(folder: &Path) -> Vec<PathBuf> {
    let entries = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"."))
            .map(|entry| entry.path())
            .collect()
    };
    let mut cards: Vec<PathBuf> = entries(folder)
        .into_iter()
        .flat_map(|path| {
            if path.is_dir() {
                entries(&path)
            } else {
                vec![path]
            }
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("vcf"))
        })
        .collect();
    cards.sort();
    cards
}

/// The name of the card in `text` that has `number`.
fn find(text: &str, number: &str) -> Option<String> {
    let mut full_name = String::new();
    let mut parts: Vec<String> = Vec::new();
    let mut matches = false;
    for line in unfold(text) {
        let Some((name, value)) = property(&line) else {
            continue;
        };
        match name.as_str() {
            "BEGIN" => {
                full_name.clear();
                parts.clear();
                matches = false;
            }
            "FN" => full_name = unescape(value).trim().to_owned(),
            // Family, given, additional, prefixes, suffixes.
            "N" => {
                parts = value
                    .split(';')
                    .map(|part| unescape(part).trim().to_owned())
                    .collect()
            }
            "TEL" => {
                // 4.0 writes a tel: URI, maybe with ;ext= after the number.
                let value = value.trim();
                let value = value
                    .get(..4)
                    .filter(|scheme| scheme.eq_ignore_ascii_case("tel:"))
                    .map_or(value, |_| &value[4..]);
                matches |= same_number(value.split(';').next().unwrap_or(value), number);
            }
            "END" if matches => {
                let name = if full_name.is_empty() {
                    [3, 1, 2, 0, 4]
                        .iter()
                        .filter_map(|&i| parts.get(i).filter(|part| !part.is_empty()))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    full_name.clone()
                };
                if !name.is_empty() {
                    return Some(name);
                }
            }
            _ => {}
        }
    }
    None
}

/// Logical lines: a line break followed by a space or a tab continues the
/// line before it.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(line)) => line.push_str(rest),
            _ => lines.push(raw.to_owned()),
        }
    }
    lines
}

/// A content line's name, upper case and without its group, and its value:
/// what follows the first colon outside quoted parameters.
fn property(line: &str) -> Option<(String, &str)> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(at, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(at),
        _ => None,
    })?;
    let head = &line[..colon];
    let head = head.split(';').next().unwrap_or(head);
    let name = head.rsplit('.').next().unwrap_or(head).to_ascii_uppercase();
    Some((name, &line[colon + 1..]))
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

/// A number's digits, without its plus and the spaces, dashes, dots,
/// parentheses and slashes people write in it.
fn digits(text: &str) -> Option<String> {
    let number = text.trim();
    let digits: String = number
        .strip_prefix('+')
        .unwrap_or(number)
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')' | '/'))
        .collect();
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

fn same_number(a: &str, b: &str) -> bool {
    match (digits(a), digits(b)) {
        (Some(a), Some(b)) if a.len().min(b.len()) >= MIN_DIGITS => {
            let n = a.len().min(b.len()).min(MATCH_DIGITS);
            a[a.len() - n..] == b[b.len() - n..]
        }
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Mario Rossi\\, jr.\r\n\
        item1.TEL;TYPE=CELL,VOICE:+39 351\r\n 123 4567\r\nEND:VCARD\r\n\
        BEGIN:VCARD\nVERSION:4.0\nN:Lovelace;Ada;;;\n\
        TEL;VALUE=uri;TYPE=\"voice,cell\":tel:+44-20-7946-0958;ext=1\nEND:VCARD\n";

    #[test]
    fn finds_names_by_number() {
        assert_eq!(
            find(CARDS, "3511234567").as_deref(),
            Some("Mario Rossi, jr.")
        );
        assert_eq!(
            find(CARDS, "+442079460958").as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(find(CARDS, "4040"), None);
    }
}

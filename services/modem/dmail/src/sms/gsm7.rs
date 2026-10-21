//! The GSM 7-bit default alphabet, its extension table, and septet packing
//! (3GPP TS 23.038).

#[rustfmt::skip]
const BASIC: [char; 128] = [
    '@', '£', '$', '¥', 'è', 'é', 'ù', 'ì', 'ò', 'Ç', '\n', 'Ø', 'ø', '\r', 'Å', 'å',
    'Δ', '_', 'Φ', 'Γ', 'Λ', 'Ω', 'Π', 'Ψ', 'Σ', 'Θ', 'Ξ', '\u{1b}', 'Æ', 'æ', 'ß', 'É',
    ' ', '!', '"', '#', '¤', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '¡', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'Ä', 'Ö', 'Ñ', 'Ü', '§',
    '¿', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'ä', 'ö', 'ñ', 'ü', 'à',
];

pub const ESC: u8 = 0x1b;

const EXTENSION: [(u8, char); 10] = [
    (0x0a, '\u{0c}'),
    (0x14, '^'),
    (0x28, '{'),
    (0x29, '}'),
    (0x2f, '\\'),
    (0x3c, '['),
    (0x3d, '~'),
    (0x3e, ']'),
    (0x40, '|'),
    (0x65, '€'),
];

/// Septets for `text`, extension characters as ESC pairs. None when a
/// character is outside the alphabet.
pub fn encode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c != '\u{1b}'
            && let Some(i) = BASIC.iter().position(|&b| b == c)
        {
            out.push(i as u8);
            continue;
        }
        let &(code, _) = EXTENSION.iter().find(|&&(_, e)| e == c)?;
        out.extend_from_slice(&[ESC, code]);
    }
    Some(out)
}

pub fn decode(septets: &[u8]) -> String {
    let mut out = String::with_capacity(septets.len());
    let mut escaped = false;
    for &s in septets {
        if escaped {
            escaped = false;
            // An unknown extension shows as the basic character (23.038).
            let c = EXTENSION.iter().find(|&&(code, _)| code == s);
            out.push(c.map_or(BASIC[(s & 0x7f) as usize], |&(_, c)| c));
        } else if s == ESC {
            escaped = true;
        } else {
            out.push(BASIC[(s & 0x7f) as usize]);
        }
    }
    out
}

/// Septets that `header` octets take at the start of user data, fill bits
/// included.
pub fn header_septets(header: usize) -> usize {
    (header * 8).div_ceil(7)
}

/// User data: `header` octets, fill bits, then `septets` packed LSB first.
pub fn pack(header: &[u8], septets: &[u8]) -> Vec<u8> {
    let start = header_septets(header.len());
    let mut out = vec![0u8; ((start + septets.len()) * 7).div_ceil(8)];
    out[..header.len()].copy_from_slice(header);
    for (i, &s) in septets.iter().enumerate() {
        let bit = (start + i) * 7;
        let v = u16::from(s & 0x7f) << (bit % 8);
        out[bit / 8] |= v as u8;
        if v > 0xff {
            out[bit / 8 + 1] |= (v >> 8) as u8;
        }
    }
    out
}

/// Septets `start..count` of packed user data. Septets the data is too short
/// for are left out.
pub fn unpack(data: &[u8], count: usize, start: usize) -> Vec<u8> {
    (start..count)
        .map_while(|i| {
            let bit = i * 7;
            let lo = *data.get(bit / 8)?;
            let hi = data.get(bit / 8 + 1).copied().unwrap_or(0);
            if bit % 8 > 1 && bit / 8 + 1 >= data.len() {
                return None;
            }
            Some(((u16::from_le_bytes([lo, hi]) >> (bit % 8)) & 0x7f) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_the_classic_example() {
        let septets = encode("hellohello").unwrap();
        let packed = pack(&[], &septets);
        assert_eq!(
            packed,
            [0xe8, 0x32, 0x9b, 0xfd, 0x46, 0x97, 0xd9, 0xec, 0x37]
        );
        assert_eq!(decode(&unpack(&packed, 10, 0)), "hellohello");
    }

    #[test]
    fn extension_characters_take_two_septets() {
        let septets = encode("€[x]").unwrap();
        assert_eq!(septets.len(), 7);
        assert_eq!(decode(&septets), "€[x]");
    }

    #[test]
    fn refuses_characters_outside_the_alphabet() {
        assert_eq!(encode("ciao 😀"), None);
        assert_eq!(encode("\u{1b}"), None);
    }

    #[test]
    fn header_and_fill_bits_come_first() {
        let header = [0x05, 0x00, 0x03, 0x2a, 0x02, 0x01];
        let septets = encode("Hi").unwrap();
        let ud = pack(&header, &septets);
        assert_eq!(&ud[..6], &header);
        assert_eq!(header_septets(6), 7);
        assert_eq!(decode(&unpack(&ud, 7 + 2, 7)), "Hi");
    }
}

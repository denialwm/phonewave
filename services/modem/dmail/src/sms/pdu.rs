//! SMS TPDUs (3GPP TS 23.040): decoding SMS-DELIVER and SMS-STATUS-REPORT,
//! encoding SMS-SUBMIT.

use std::fmt;

use super::gsm7;

/// One incoming message, or one part of a long one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deliver {
    pub sender: String,
    /// The service centre time stamp, in Unix seconds. None when it is not a
    /// valid date.
    pub time: Option<i64>,
    pub text: String,
    pub part: Option<Part>,
}

/// A delivery report for one sent part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReport {
    /// The message reference the part went out with.
    pub reference: u8,
    pub recipient: String,
    /// TP-Status, section 9.2.3.15.
    pub status: u8,
}

/// TP-Status: the validity period ran out before the recipient took the
/// message.
const VALIDITY_EXPIRED: u8 = 0x46;

impl StatusReport {
    /// The state the report gives the part: delivered, expired or
    /// undelivered. None while the service centre keeps trying; another
    /// report follows.
    pub fn outcome(&self) -> Option<&'static str> {
        match self.status {
            0x00..=0x1f => Some("delivered"),
            0x20..=0x3f => None,
            VALIDITY_EXPIRED => Some("expired"),
            _ => Some("undelivered"),
        }
    }
}

/// A TPDU the network sends to the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tpdu {
    Deliver(Deliver),
    StatusReport(StatusReport),
}

/// Where a part sits in a long message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub reference: u16,
    pub total: u8,
    pub sequence: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    /// Neither an SMS-DELIVER nor an SMS-STATUS-REPORT; carries the first
    /// octet.
    UnknownType(u8),
    /// A data coding scheme dmail cannot read, such as compressed text.
    Coding(u8),
    BadNumber,
    TooLong,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => f.write_str("truncated SMS"),
            Error::UnknownType(first) => write!(f, "unknown SMS type (first octet {first:#04x})"),
            Error::Coding(dcs) => write!(f, "unsupported data coding {dcs:#04x}"),
            Error::BadNumber => f.write_str("not a phone number"),
            Error::TooLong => f.write_str("text needs more than 255 parts"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coding {
    Gsm7,
    Octets,
    Ucs2,
}

/// The alphabet of a data coding scheme (23.038 section 4).
fn coding(dcs: u8) -> Result<Coding, Error> {
    let general = |dcs: u8| match (dcs >> 2) & 3 {
        _ if dcs & 0x20 != 0 => Err(Error::Coding(dcs)),
        0 => Ok(Coding::Gsm7),
        1 => Ok(Coding::Octets),
        2 => Ok(Coding::Ucs2),
        _ => Err(Error::Coding(dcs)),
    };
    match dcs >> 4 {
        0x0..=0x7 => general(dcs),
        0xc | 0xd => Ok(Coding::Gsm7),
        0xe => Ok(Coding::Ucs2),
        0xf if dcs & 0x04 != 0 => Ok(Coding::Octets),
        0xf => Ok(Coding::Gsm7),
        _ => Err(Error::Coding(dcs)),
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
}

const DIGITS: &[u8; 16] = b"0123456789*#abc?";

fn decode_address(r: &mut Reader) -> Result<String, Error> {
    let len = r.u8()? as usize;
    let toa = r.u8()?;
    let bytes = r.take(len.div_ceil(2))?;
    if toa & 0x70 == 0x50 {
        // Alphanumeric: `len` counts semi-octets of packed septets.
        return Ok(gsm7::decode(&gsm7::unpack(bytes, len * 4 / 7, 0)));
    }
    let mut out = String::with_capacity(len + 1);
    if toa & 0x70 == 0x10 {
        out.push('+');
    }
    for i in 0..len {
        let nibble = (bytes[i / 2] >> (4 * (i % 2))) & 0x0f;
        out.push(DIGITS[nibble as usize] as char);
    }
    Ok(out)
}

fn decode_time(stamp: &[u8]) -> Option<i64> {
    let bcd = |b: u8| (b & 0x0f <= 9 && b >> 4 <= 9).then(|| i64::from((b & 0x0f) * 10 + (b >> 4)));
    let (year, month, day) = (bcd(stamp[0])?, bcd(stamp[1])?, bcd(stamp[2])?);
    let (hour, minute, second) = (bcd(stamp[3])?, bcd(stamp[4])?, bcd(stamp[5])?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // The zone is in quarter hours; bit 3 of the first semi-octet is its sign.
    let tz = stamp[6];
    let quarters = i64::from((tz & 0x07) * 10 + (tz >> 4));
    let offset = if tz & 0x08 != 0 { -quarters } else { quarters } * 900;
    let days = days_from_civil(2000 + year, month, day);
    Some(days * 86400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn decode_part(header: &[u8]) -> Option<Part> {
    let mut rest = header;
    while let [id, len, tail @ ..] = rest {
        let (value, next) = tail.split_at_checked(*len as usize)?;
        match (*id, value) {
            (0x00, &[reference, total, sequence]) => {
                let reference = u16::from(reference);
                return Some(Part {
                    reference,
                    total,
                    sequence,
                });
            }
            (0x08, &[r0, r1, total, sequence]) => {
                let reference = u16::from_be_bytes([r0, r1]);
                return Some(Part {
                    reference,
                    total,
                    sequence,
                });
            }
            _ => rest = next,
        }
    }
    None
}

/// Decodes a TPDU from the network, with no SMSC address in front.
pub fn decode(tpdu: &[u8]) -> Result<Tpdu, Error> {
    let first = *tpdu.first().ok_or(Error::Truncated)?;
    match first & 0x03 {
        0 => decode_deliver(tpdu).map(Tpdu::Deliver),
        2 => decode_status_report(tpdu).map(Tpdu::StatusReport),
        _ => Err(Error::UnknownType(first)),
    }
}

fn decode_deliver(tpdu: &[u8]) -> Result<Deliver, Error> {
    let mut r = Reader(tpdu);
    let first = r.u8()?;
    let sender = decode_address(&mut r)?;
    let _pid = r.u8()?;
    let dcs = r.u8()?;
    let time = decode_time(r.take(7)?);
    let udl = r.u8()? as usize;
    let ud = r.0;
    let has_header = first & 0x40 != 0;
    let header_len = match (has_header, ud.first()) {
        (true, Some(&udhl)) => 1 + udhl as usize,
        (true, None) => return Err(Error::Truncated),
        (false, _) => 0,
    };
    let header = match header_len {
        0 => &[][..],
        n => ud.get(1..n).ok_or(Error::Truncated)?,
    };
    let text = match coding(dcs)? {
        Coding::Gsm7 => {
            let skip = gsm7::header_septets(header_len);
            gsm7::decode(&gsm7::unpack(ud, udl, skip))
        }
        coding => {
            let body = ud
                .get(header_len..udl.min(ud.len()))
                .ok_or(Error::Truncated)?;
            if coding == Coding::Ucs2 {
                let units: Vec<u16> = body
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                body.iter().map(|&b| char::from(b)).collect()
            }
        }
    };
    Ok(Deliver {
        sender,
        time,
        text,
        part: decode_part(header),
    })
}

fn decode_status_report(tpdu: &[u8]) -> Result<StatusReport, Error> {
    let mut r = Reader(tpdu);
    let _first = r.u8()?;
    let reference = r.u8()?;
    let recipient = decode_address(&mut r)?;
    // The service centre's time stamp and the time of the outcome.
    r.take(14)?;
    let status = r.u8()?;
    Ok(StatusReport {
        reference,
        recipient,
        status,
    })
}

/// The TPDU of a message read from storage, past the SMSC address in front.
pub fn skip_smsc(pdu: &[u8]) -> Result<&[u8], Error> {
    let len = *pdu.first().ok_or(Error::Truncated)? as usize;
    pdu.get(1 + len..).ok_or(Error::Truncated)
}

fn encode_address(number: &str) -> Result<Vec<u8>, Error> {
    let (toa, digits) = match number.strip_prefix('+') {
        Some(rest) => (0x91, rest),
        None => (0x81, number),
    };
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::BadNumber);
    }
    let mut out = vec![digits.len() as u8, toa];
    for pair in digits.as_bytes().chunks(2) {
        let lo = pair[0] - b'0';
        let hi = pair.get(1).map_or(0x0f, |d| d - b'0');
        out.push(lo | hi << 4);
    }
    Ok(out)
}

const SINGLE_SEPTETS: usize = 160;
const PART_SEPTETS: usize = 153;
const SINGLE_UNITS: usize = 70;
const PART_UNITS: usize = 67;

/// Splits `items` into chunks of at most `max`, never ending a chunk on an
/// item `keep_with_next` says must stay with the one after it.
fn chunks<T: Copy>(items: &[T], max: usize, keep_with_next: impl Fn(T) -> bool) -> Vec<&[T]> {
    let mut out = Vec::new();
    let mut rest = items;
    while rest.len() > max {
        let mut end = max;
        if keep_with_next(rest[end - 1]) {
            end -= 1;
        }
        let (head, tail) = rest.split_at(end);
        out.push(head);
        rest = tail;
    }
    out.push(rest);
    out
}

/// SMS-SUBMIT TPDUs for `text` to `number`. Each starts with an empty SMSC
/// field, so the modem uses the SIM's service centre, and message reference
/// 0, which the modem replaces with its own. A long text becomes several
/// parts joined by `reference`. With `report`, every part asks for a status
/// report.
pub fn encode_submit(
    number: &str,
    text: &str,
    reference: u8,
    report: bool,
) -> Result<Vec<Vec<u8>>, Error> {
    let address = encode_address(number)?;
    let tpdu = |dcs: u8, header: Option<[u8; 6]>, udl: usize, ud: Vec<u8>| {
        // TP-MTI SUBMIT, then TP-SRR and TP-UDHI.
        let mut first = 0x01;
        if report {
            first |= 0x20;
        }
        if header.is_some() {
            first |= 0x40;
        }
        let mut out = vec![0x00, first, 0x00];
        out.extend_from_slice(&address);
        out.extend_from_slice(&[0x00, dcs, udl as u8]);
        out.extend(ud);
        out
    };
    let header =
        |total: usize, sequence: usize| [0x05, 0x00, 0x03, reference, total as u8, sequence as u8];

    if let Some(septets) = gsm7::encode(text) {
        if septets.len() <= SINGLE_SEPTETS {
            return Ok(vec![tpdu(
                0x00,
                None,
                septets.len(),
                gsm7::pack(&[], &septets),
            )]);
        }
        let parts = chunks(&septets, PART_SEPTETS, |s| s == gsm7::ESC);
        if parts.len() > 255 {
            return Err(Error::TooLong);
        }
        let total = parts.len();
        return Ok(parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let h = header(total, i + 1);
                let udl = gsm7::header_septets(h.len()) + part.len();
                tpdu(0x00, Some(h), udl, gsm7::pack(&h, part))
            })
            .collect());
    }

    let units: Vec<u16> = text.encode_utf16().collect();
    let be = |units: &[u16]| {
        units
            .iter()
            .flat_map(|u| u.to_be_bytes())
            .collect::<Vec<u8>>()
    };
    if units.len() <= SINGLE_UNITS {
        return Ok(vec![tpdu(0x08, None, units.len() * 2, be(&units))]);
    }
    let parts = chunks(&units, PART_UNITS, |u| (0xd800..0xdc00).contains(&u));
    if parts.len() > 255 {
        return Err(Error::TooLong);
    }
    let total = parts.len();
    Ok(parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let h = header(total, i + 1);
            let mut ud = h.to_vec();
            ud.extend(be(part));
            tpdu(0x08, Some(h), ud.len(), ud)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-21 12:34:56 at UTC+2, semi-octets swapped.
    const STAMP: [u8; 7] = [0x62, 0x01, 0x12, 0x21, 0x43, 0x65, 0x80];
    /// The same moment in Unix seconds: 2026-10-21 10:34:56 UTC.
    const STAMP_UNIX: i64 = 1_792_578_896;

    /// Turns one of our SMS-SUBMITs into the SMS-DELIVER a receiver would
    /// get, so encoding can be checked by decoding.
    fn as_delivered(submit: &[u8]) -> Vec<u8> {
        assert_eq!(submit[0], 0, "empty SMSC field");
        let first = submit[1];
        let address_len = 2 + (submit[3] as usize).div_ceil(2);
        let address = &submit[3..3 + address_len];
        let rest = &submit[3 + address_len..];
        let (pid, dcs, user_data) = (rest[0], rest[1], &rest[2..]);
        let mut out = vec![first & 0x40];
        out.extend_from_slice(address);
        out.extend_from_slice(&[pid, dcs]);
        out.extend_from_slice(&STAMP);
        out.extend_from_slice(user_data);
        out
    }

    fn round_trip(number: &str, text: &str) -> Vec<Deliver> {
        encode_submit(number, text, 42, false)
            .unwrap()
            .iter()
            .map(|p| match decode(&as_delivered(p)).unwrap() {
                Tpdu::Deliver(d) => d,
                other => panic!("not delivered: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn decodes_a_plain_deliver() {
        let mut tpdu = vec![
            0x04, 11, 0x91, 0x21, 0x43, 0x65, 0x87, 0x09, 0xf1, 0x00, 0x00,
        ];
        tpdu.extend_from_slice(&STAMP);
        tpdu.push(10);
        tpdu.extend(gsm7::pack(&[], &gsm7::encode("hellohello").unwrap()));
        let Tpdu::Deliver(d) = decode(&tpdu).unwrap() else {
            panic!("not an SMS-DELIVER");
        };
        assert_eq!(d.sender, "+12345678901");
        assert_eq!(d.text, "hellohello");
        assert_eq!(d.time, Some(STAMP_UNIX));
        assert_eq!(d.part, None);
    }

    #[test]
    fn decodes_an_alphanumeric_sender() {
        let name = gsm7::pack(&[], &gsm7::encode("Info").unwrap());
        let mut tpdu = vec![0x00, 7, 0xd0];
        tpdu.extend(&name);
        tpdu.extend_from_slice(&[0x00, 0x08]);
        tpdu.extend_from_slice(&STAMP);
        tpdu.extend_from_slice(&[4, 0x00, 0x4f, 0x00, 0x4b]);
        let Tpdu::Deliver(d) = decode(&tpdu).unwrap() else {
            panic!("not an SMS-DELIVER");
        };
        assert_eq!(d.sender, "Info");
        assert_eq!(d.text, "OK");
    }

    #[test]
    fn short_texts_are_one_part() {
        let parts = encode_submit("+391234567", "Ciao!", 1, false).unwrap();
        assert_eq!(parts.len(), 1);
        let d = &round_trip("+391234567", "Ciao!")[0];
        assert_eq!(
            (d.sender.as_str(), d.text.as_str(), d.part),
            ("+391234567", "Ciao!", None)
        );
    }

    #[test]
    fn splits_at_160_septets() {
        assert_eq!(
            encode_submit("123", &"a".repeat(160), 0, false)
                .unwrap()
                .len(),
            1
        );
        let text = "b".repeat(161);
        let got = round_trip("123", &text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text.len(), 153);
        assert_eq!(
            got[1].part,
            Some(Part {
                reference: 42,
                total: 2,
                sequence: 2
            })
        );
        assert_eq!(
            got.iter().map(|d| d.text.as_str()).collect::<String>(),
            text
        );
    }

    #[test]
    fn never_splits_an_escape_pair() {
        let text = format!("{}€{}", "x".repeat(152), "y".repeat(20));
        let got = round_trip("123", &text);
        assert_eq!(got[0].text, "x".repeat(152));
        assert_eq!(
            got.iter().map(|d| d.text.as_str()).collect::<String>(),
            text
        );
    }

    #[test]
    fn falls_back_to_ucs2_and_keeps_surrogate_pairs() {
        let single = round_trip("123", "Ciao 😀");
        assert_eq!(single[0].text, "Ciao 😀");
        let text = format!("{}😀{}", "z".repeat(66), "w".repeat(10));
        let got = round_trip("123", &text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "z".repeat(66));
        assert_eq!(
            got.iter().map(|d| d.text.as_str()).collect::<String>(),
            text
        );
    }

    #[test]
    fn rejects_bad_numbers() {
        assert_eq!(encode_submit("", "x", 0, false), Err(Error::BadNumber));
        assert_eq!(
            encode_submit("+39 333", "x", 0, false),
            Err(Error::BadNumber)
        );
        assert_eq!(encode_submit("12a", "x", 0, false), Err(Error::BadNumber));
    }

    #[test]
    fn reads_codings() {
        assert_eq!(coding(0x00), Ok(Coding::Gsm7));
        assert_eq!(coding(0x08), Ok(Coding::Ucs2));
        assert_eq!(coding(0x04), Ok(Coding::Octets));
        assert_eq!(coding(0x20), Err(Error::Coding(0x20)));
        assert_eq!(coding(0xf1), Ok(Coding::Gsm7));
        assert_eq!(coding(0xf4), Ok(Coding::Octets));
        assert_eq!(coding(0xe0), Ok(Coding::Ucs2));
    }

    #[test]
    fn rejects_other_message_types() {
        assert_eq!(decode(&[0x01, 0x00]), Err(Error::UnknownType(0x01)));
        assert_eq!(decode(&[0x00, 0x05]), Err(Error::Truncated));
        assert_eq!(decode(&[]), Err(Error::Truncated));
    }

    /// An SMS-STATUS-REPORT for reference 42 to +12345678901.
    fn status_report(status: u8) -> Vec<u8> {
        let mut tpdu = vec![0x06, 42, 11, 0x91, 0x21, 0x43, 0x65, 0x87, 0x09, 0xf1];
        tpdu.extend_from_slice(&STAMP);
        tpdu.extend_from_slice(&STAMP);
        tpdu.push(status);
        tpdu
    }

    #[test]
    fn decodes_a_status_report() {
        let Tpdu::StatusReport(r) = decode(&status_report(0x00)).unwrap() else {
            panic!("not a status report");
        };
        assert_eq!(
            (r.reference, r.recipient.as_str(), r.outcome()),
            (42, "+12345678901", Some("delivered"))
        );
        let mut short = status_report(0x00);
        short.pop();
        assert_eq!(decode(&short), Err(Error::Truncated));
    }

    #[test]
    fn reads_report_outcomes() {
        let outcome = |status| match decode(&status_report(status)).unwrap() {
            Tpdu::StatusReport(r) => r.outcome(),
            other => panic!("not a status report: {other:?}"),
        };
        // Forwarded but unconfirmed still counts as delivered.
        assert_eq!(outcome(0x01), Some("delivered"));
        // Recipient busy, the centre keeps trying.
        assert_eq!(outcome(0x21), None);
        assert_eq!(outcome(VALIDITY_EXPIRED), Some("expired"));
        // Incompatible destination; recipient busy and the centre gave up.
        assert_eq!(outcome(0x41), Some("undelivered"));
        assert_eq!(outcome(0x61), Some("undelivered"));
    }

    #[test]
    fn asks_for_reports_on_every_part() {
        let first_octets = |report| {
            encode_submit("123", &"r".repeat(200), 7, report)
                .unwrap()
                .iter()
                .map(|p| (p[1], p[2]))
                .collect::<Vec<_>>()
        };
        // TP-SRR on top of SUBMIT and the user data header; reference 0.
        assert_eq!(first_octets(true), [(0x61, 0), (0x61, 0)]);
        assert_eq!(first_octets(false), [(0x41, 0), (0x41, 0)]);
        assert_eq!(encode_submit("123", "x", 7, true).unwrap()[0][1], 0x21);
    }

    #[test]
    fn bad_time_stamps_are_none() {
        let mut stamp = STAMP;
        stamp[1] = 0x31; // month 13
        assert_eq!(decode_time(&stamp), None);
    }
}

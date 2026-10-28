//! The modem's voice service: call notifications, dial, answer, end, DTMF.

use std::time::Duration;

use async_io::Timer;

use crate::qmi::{Client, Error, Message};

const INDICATION_REGISTER: u16 = 0x03;
const DIAL: u16 = 0x20;
const END: u16 = 0x21;
const ANSWER: u16 = 0x22;
const START_DTMF: u16 = 0x29;
const STOP_DTMF: u16 = 0x2a;
pub const ALL_CALL_STATUS_IND: u16 = 0x2e;
const ALL_CALL_INFO: u16 = 0x2f;
const TIMEOUT: Duration = Duration::from_secs(10);
/// How long each DTMF digit sounds, and the pause after it.
const DTMF_TONE: Duration = Duration::from_millis(120);
const DTMF_GAP: Duration = Duration::from_millis(80);

/// One call as the modem lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: u8,
    pub state: State,
    pub incoming: bool,
    /// Empty when the caller withheld it.
    pub number: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Dialing,
    /// The other phone rings.
    Alerting,
    Incoming,
    /// Incoming while another call is active.
    Waiting,
    Active,
    Held,
    Ending,
    Ended,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Dialing => "dialing",
            State::Alerting => "alerting",
            State::Incoming => "incoming",
            State::Waiting => "waiting",
            State::Active => "active",
            State::Held => "held",
            State::Ending => "ending",
            State::Ended => "ended",
        }
    }

    /// QMI call states: origination, incoming, conversation, cc-in-progress,
    /// alerting, hold, waiting, disconnecting, end, setup.
    fn from_qmi(state: u8, incoming: bool) -> Self {
        match state {
            2 => State::Incoming,
            3 => State::Active,
            5 => State::Alerting,
            6 => State::Held,
            7 => State::Waiting,
            8 => State::Ending,
            9 => State::Ended,
            10 if incoming => State::Incoming,
            _ => State::Dialing,
        }
    }
}

/// Asks for call status indications.
pub async fn report_calls(voice: &Client) -> Result<(), Error> {
    let msg = Message::request(INDICATION_REGISTER).with(0x13, [1]);
    voice.call(msg, TIMEOUT).await.map(drop)
}

/// The calls the modem has now, ended ones left out.
pub async fn calls(voice: &Client) -> Result<Vec<Call>, Error> {
    let reply = voice.call(Message::request(ALL_CALL_INFO), TIMEOUT).await?;
    let mut calls = parse_calls(&reply, 0x10, 0x11);
    calls.retain(|c| c.state != State::Ended);
    Ok(calls)
}

/// Every call in an All Call Status indication, the ending ones included.
pub fn parse_status(msg: &Message) -> Vec<Call> {
    parse_calls(msg, 0x01, 0x10)
}

/// Call information records are 7 bytes: id, state, type, direction, mode,
/// multipart, ALS. Numbers come as id, presentation, length, digits.
fn parse_calls(msg: &Message, info: u8, numbers: u8) -> Vec<Call> {
    let mut calls: Vec<Call> = match msg.tlv(info) {
        Some([count, records @ ..]) => records
            .chunks_exact(7)
            .take(usize::from(*count))
            .map(|r| Call {
                id: r[0],
                state: State::from_qmi(r[1], r[3] == 2),
                incoming: r[3] == 2,
                number: String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    if let Some([count, list @ ..]) = msg.tlv(numbers) {
        let mut rest = list;
        for _ in 0..*count {
            let &[id, _presentation, len, ref tail @ ..] = rest else {
                break;
            };
            let Some(digits) = tail.get(..usize::from(len)) else {
                break;
            };
            if let Some(call) = calls.iter_mut().find(|c| c.id == id) {
                call.number = String::from_utf8_lossy(digits).into_owned();
            }
            rest = &tail[usize::from(len)..];
        }
    }
    calls
}

/// Starts a call; its id follows the modem's calls from here on.
pub async fn dial(voice: &Client, number: &str) -> Result<u8, Error> {
    let reply = voice
        .call(
            Message::request(DIAL).with(0x01, number.as_bytes()),
            TIMEOUT,
        )
        .await?;
    match reply.tlv(0x10) {
        Some(&[id]) => Ok(id),
        _ => Err(Error::Malformed("dial answer without a call id")),
    }
}

pub async fn answer(voice: &Client, id: u8) -> Result<(), Error> {
    let msg = Message::request(ANSWER).with(0x01, [id]);
    voice.call(msg, TIMEOUT).await.map(drop)
}

/// Hangs up, or turns down an incoming call.
pub async fn end(voice: &Client, id: u8) -> Result<(), Error> {
    let msg = Message::request(END).with(0x01, [id]);
    voice.call(msg, TIMEOUT).await.map(drop)
}

/// Plays `digits` to the other side one after the other.
pub async fn dtmf(voice: &Client, id: u8, digits: &str) -> Result<(), Error> {
    for digit in digits.bytes() {
        let start = Message::request(START_DTMF).with(0x01, [id, digit]);
        voice.call(start, TIMEOUT).await?;
        Timer::after(DTMF_TONE).await;
        let stop = Message::request(STOP_DTMF).with(0x01, [id]);
        voice.call(stop, TIMEOUT).await?;
        Timer::after(DTMF_GAP).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qmi::Kind;

    fn indication(tlvs: &[(u8, &[u8])]) -> Message {
        let mut msg = Message::request(ALL_CALL_STATUS_IND);
        msg.kind = Kind::Indication;
        tlvs.iter().fold(msg, |m, (t, v)| m.with(*t, *v))
    }

    #[test]
    fn reads_an_incoming_call() {
        // As aston's modem sent it on 2026-10-21: call 1, incoming, voice,
        // mobile terminated, UMTS.
        let mut numbers = vec![1, 1, 0, 13];
        numbers.extend(b"+393279994733");
        let msg = indication(&[(0x01, &[1, 1, 2, 0, 2, 3, 0, 0]), (0x10, &numbers)]);
        assert_eq!(
            parse_status(&msg),
            [Call {
                id: 1,
                state: State::Incoming,
                incoming: true,
                number: "+393279994733".into(),
            }]
        );
    }

    #[test]
    fn reads_states() {
        let msg = indication(&[(0x01, &[2, 1, 5, 0, 1, 3, 0, 0, 2, 7, 0, 2, 3, 0, 0])]);
        let calls = parse_status(&msg);
        assert_eq!(calls[0].state, State::Alerting);
        assert!(!calls[0].incoming);
        assert_eq!(calls[1].state, State::Waiting);
        assert_eq!(calls[1].number, "");
    }

    #[test]
    fn survives_a_short_number_list() {
        let msg = indication(&[
            (0x01, &[1, 1, 3, 0, 1, 3, 0, 0]),
            (0x10, &[1, 1, 0, 9, b'1']),
        ]);
        assert_eq!(parse_status(&msg)[0].number, "");
    }
}

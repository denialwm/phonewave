//! alsa-lib and libpipewire, through src/native.c.

use std::ffi::{CStr, CString, c_char, c_int, c_long};
use std::io;

unsafe extern "C" {
    fn callaudio_ucm_find(
        verb: *const c_char,
        card: *mut c_int,
        volume: *mut c_char,
        volume_len: usize,
        mute: *mut c_char,
        mute_len: usize,
    ) -> c_int;
    fn callaudio_ctl_get(
        card: c_int,
        id: *const c_char,
        value: *mut c_long,
        min: *mut c_long,
        max: *mut c_long,
    ) -> c_int;
    fn callaudio_ctl_set(card: c_int, id: *const c_char, value: c_long) -> c_int;
    fn callaudio_pw_profile(
        card: c_int,
        want: *const c_char,
        previous: *mut c_char,
        previous_len: usize,
    ) -> c_int;
}

fn check(ret: c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::from_raw_os_error(-ret))
    } else {
        Ok(())
    }
}

fn c(text: &str) -> io::Result<CString> {
    CString::new(text).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn text(buffer: &[u8]) -> String {
    CStr::from_bytes_until_nul(buffer)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A sound card with a voice call verb, and the controls the verb's devices
/// name: `PlaybackVolume` and `CaptureSwitch`, empty when it names none.
#[derive(Debug)]
pub struct Card {
    pub index: i32,
    pub volume: String,
    pub mute: String,
}

pub fn find_card(verb: &str) -> io::Result<Card> {
    let verb = c(verb)?;
    let (mut index, mut volume, mut mute) = (0, [0u8; 256], [0u8; 256]);
    check(unsafe {
        callaudio_ucm_find(
            verb.as_ptr(),
            &mut index,
            volume.as_mut_ptr().cast(),
            volume.len(),
            mute.as_mut_ptr().cast(),
            mute.len(),
        )
    })?;
    Ok(Card {
        index,
        volume: text(&volume),
        mute: text(&mute),
    })
}

/// A control's value and range.
pub struct Control {
    pub value: i64,
    pub min: i64,
    pub max: i64,
}

pub fn get(card: i32, id: &str) -> io::Result<Control> {
    let id = c(id)?;
    let (mut value, mut min, mut max) = (0, 0, 0);
    check(unsafe { callaudio_ctl_get(card, id.as_ptr(), &mut value, &mut min, &mut max) })?;
    Ok(Control { value, min, max })
}

pub fn set(card: i32, id: &str, value: i64) -> io::Result<()> {
    let id = c(id)?;
    check(unsafe { callaudio_ctl_set(card, id.as_ptr(), value) })
}

/// Puts the card's PipeWire device on `profile` for now, without making it
/// the stored choice. Returns the profile it had.
pub fn set_profile(card: i32, profile: &str) -> io::Result<String> {
    let profile = c(profile)?;
    let mut previous = [0u8; 256];
    check(unsafe {
        callaudio_pw_profile(
            card,
            profile.as_ptr(),
            previous.as_mut_ptr().cast(),
            previous.len(),
        )
    })?;
    Ok(text(&previous))
}

//! The account password, through PAM's `login` stack as the lock screen
//! checks it. libpam is loaded when a reset asks for it.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

#[repr(C)]
struct Message {
    style: c_int,
    text: *const c_char,
}
#[repr(C)]
struct Response {
    text: *mut c_char,
    code: c_int,
}
#[repr(C)]
struct Conversation {
    call:
        unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int,
    data: *mut c_void,
}
type Start = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    *const Conversation,
    *mut *mut c_void,
) -> c_int;
type Step = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type SetItem = unsafe extern "C" fn(*mut c_void, c_int, *const c_void) -> c_int;

const SUCCESS: c_int = 0;
const BUF_ERR: c_int = 5;
const CONV_ERR: c_int = 19;
const AUTH_ERR: c_int = 7;
const CRED_INSUFFICIENT: c_int = 8;
const AUTHINFO_UNAVAIL: c_int = 9;
const USER_UNKNOWN: c_int = 10;
const MAXTRIES: c_int = 11;
const PROMPT_ECHO_OFF: c_int = 1;
const PROMPT_ECHO_ON: c_int = 2;
const TTY: c_int = 3;
const SILENT: c_int = 0x8000;
const DISALLOW_NULL_AUTHTOK: c_int = 0x0001;
const MAX_MESSAGES: usize = 32;

/// Frees the first `count` answers and the array.
unsafe fn free_replies(replies: *mut Response, count: usize) {
    for index in 0..count {
        // SAFETY: the caller's array holds at least `count` entries.
        unsafe { libc::free((*replies.add(index)).text.cast()) };
    }
    // SAFETY: allocated with calloc.
    unsafe { libc::free(replies.cast()) };
}

/// Answers every hidden prompt with the password and nothing else.
unsafe extern "C" fn converse(
    count: c_int,
    messages: *mut *const Message,
    responses: *mut *mut Response,
    data: *mut c_void,
) -> c_int {
    let Some(count) = usize::try_from(count)
        .ok()
        .filter(|count| (1..=MAX_MESSAGES).contains(count))
    else {
        return CONV_ERR;
    };
    if messages.is_null() || responses.is_null() {
        return CONV_ERR;
    }
    // SAFETY: data is the password CString `check` passes.
    let password = unsafe { &*data.cast::<CString>() };
    // SAFETY: PAM frees the array and the strings it holds with free().
    let replies =
        unsafe { libc::calloc(count, std::mem::size_of::<Response>()) }.cast::<Response>();
    if replies.is_null() {
        return BUF_ERR;
    }
    for index in 0..count {
        // SAFETY: PAM passes `count` message pointers.
        let message = unsafe { *messages.add(index) };
        if message.is_null() {
            continue;
        }
        // SAFETY: a valid message from PAM.
        let style = unsafe { (*message).style };
        if style == PROMPT_ECHO_OFF {
            // SAFETY: a NUL-terminated copy PAM frees.
            let copy = unsafe { libc::strdup(password.as_ptr()) };
            if copy.is_null() {
                // SAFETY: `index` entries are zeroed or filled.
                unsafe { free_replies(replies, index) };
                return BUF_ERR;
            }
            // SAFETY: index < count.
            unsafe { (*replies.add(index)).text = copy };
        } else if style == PROMPT_ECHO_ON {
            // Nobody types a visible answer here, such as another user name.
            // SAFETY: as above.
            unsafe { free_replies(replies, index) };
            return CONV_ERR;
        }
    }
    // SAFETY: PAM owns the array from here on.
    unsafe { *responses = replies };
    SUCCESS
}

fn symbols(library: *mut c_void) -> Result<(Start, Step, Step, Step, SetItem), String> {
    let symbol = |name: &CStr| {
        // SAFETY: a live handle and a NUL-terminated name.
        let address = unsafe { libc::dlsym(library, name.as_ptr()) };
        (!address.is_null())
            .then_some(address)
            .ok_or_else(|| format!("libpam lacks {name:?}"))
    };
    // SAFETY: each symbol has the C signature libpam documents.
    unsafe {
        Ok((
            std::mem::transmute::<*mut c_void, Start>(symbol(c"pam_start")?),
            std::mem::transmute::<*mut c_void, Step>(symbol(c"pam_end")?),
            std::mem::transmute::<*mut c_void, Step>(symbol(c"pam_authenticate")?),
            std::mem::transmute::<*mut c_void, Step>(symbol(c"pam_acct_mgmt")?),
            std::mem::transmute::<*mut c_void, SetItem>(symbol(c"pam_set_item")?),
        ))
    }
}

/// Whether `password` is `user`'s account password and the account may log
/// in. Err when PAM itself cannot answer.
pub fn check(user: &CStr, password: &[u8]) -> Result<bool, String> {
    // SAFETY: a fixed SONAME; the process ends after this one request.
    let library = unsafe { libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW) };
    if library.is_null() {
        return Err("libpam.so.0 is not installed".into());
    }
    let (start, end, authenticate, account, set_item) = symbols(library)?;
    let Ok(mut password) = CString::new(password) else {
        return Ok(false);
    };
    let conversation = Conversation {
        call: converse,
        data: (&raw mut password).cast(),
    };
    let mut handle = std::ptr::null_mut();
    // SAFETY: the service, user and conversation outlive the balanced pam_end.
    let mut result = unsafe {
        start(
            c"login".as_ptr(),
            user.as_ptr(),
            &raw const conversation,
            &raw mut handle,
        )
    };
    let checked = if result != SUCCESS || handle.is_null() {
        Err(format!("PAM did not start: {result}"))
    } else {
        // SAFETY: handle is live until the pam_end below.
        unsafe {
            set_item(handle, TTY, c"phonewave-gatekeeper".as_ptr().cast());
            result = authenticate(handle, DISALLOW_NULL_AUTHTOK);
            if result == SUCCESS {
                result = account(handle, SILENT);
            }
            end(handle, result);
        }
        match result {
            SUCCESS => Ok(true),
            AUTH_ERR | USER_UNKNOWN | MAXTRIES | CRED_INSUFFICIENT | AUTHINFO_UNAVAIL => Ok(false),
            other => Err(format!("PAM failed: {other}")),
        }
    };
    let mut bytes = password.into_bytes();
    phonewave_gatekeeper::erase(&mut bytes);
    checked
}

//! C ABI for Swift. Declared in `include/ob_mobile_tor.h`.
//!
//! Strings in are NUL-terminated UTF-8. Every string or buffer handed out
//! must be freed with `ob_tor_string_free` / `ob_tor_bytes_free`.

use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;
use std::time::Duration;

#[repr(C)]
pub struct ObTorBytes {
    pub data: *mut u8,
    pub len: usize,
    /// NULL on success; otherwise the reason, and `data` is NULL.
    pub error: *mut c_char,
}

unsafe fn text<'a>(value: *const c_char) -> Result<&'a str, String> {
    if value.is_null() {
        return Err("missing argument".into());
    }
    CStr::from_ptr(value)
        .to_str()
        .map_err(|_| "argument is not UTF-8".into())
}

fn owned(text: String) -> *mut c_char {
    CString::new(text.replace('\0', " "))
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

fn outcome(result: Result<(), String>) -> *mut c_char {
    match result {
        Ok(()) => ptr::null_mut(),
        Err(err) => owned(err),
    }
}

/// Start Tor with its files under `data_dir`. NULL on success, else the error.
/// # Safety
/// `data_dir` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn ob_tor_start(data_dir: *const c_char) -> *mut c_char {
    outcome(text(data_dir).and_then(|dir| crate::start(Path::new(dir))))
}

/// `{"state":"stopped"|"bootstrapping"|"ready"|"failed","progress":0..1,"summary":"…"}`
#[no_mangle]
pub extern "C" fn ob_tor_status() -> *mut c_char {
    owned(crate::status().to_json())
}

/// Store the restricted-discovery secret (base32) for `onion`. NULL on success.
/// # Safety
/// Both arguments are NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn ob_tor_insert_discovery_key(
    onion: *const c_char,
    secret_base32: *const c_char,
) -> *mut c_char {
    outcome(text(onion).and_then(|onion| crate::insert_discovery_key(onion, text(secret_base32)?)))
}

/// Send `request` to `onion:port` on a fresh stream and return what came back.
/// # Safety
/// `onion` is a NUL-terminated string and `request` points to `request_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn ob_tor_exchange(
    onion: *const c_char,
    port: u16,
    request: *const u8,
    request_len: usize,
    max_bytes: usize,
    timeout_ms: u64,
) -> ObTorBytes {
    let result = text(onion).and_then(|onion| {
        if request.is_null() {
            return Err("missing request".to_string());
        }
        let request = std::slice::from_raw_parts(request, request_len);
        crate::exchange(
            onion,
            port,
            request,
            max_bytes,
            Duration::from_millis(timeout_ms),
        )
    });
    match result {
        Ok(bytes) => {
            let mut bytes = bytes.into_boxed_slice();
            let out = ObTorBytes {
                data: bytes.as_mut_ptr(),
                len: bytes.len(),
                error: ptr::null_mut(),
            };
            std::mem::forget(bytes);
            out
        }
        Err(err) => ObTorBytes {
            data: ptr::null_mut(),
            len: 0,
            error: owned(err),
        },
    }
}

/// Free a string this library returned.
/// # Safety
/// `value` is NULL or came from this library and is freed once.
#[no_mangle]
pub unsafe extern "C" fn ob_tor_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

/// Free an answer from `ob_tor_exchange`.
/// # Safety
/// `value` came from `ob_tor_exchange` and is freed once.
#[no_mangle]
pub unsafe extern "C" fn ob_tor_bytes_free(value: ObTorBytes) {
    if !value.data.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            value.data, value.len,
        )));
    }
    ob_tor_string_free(value.error);
}

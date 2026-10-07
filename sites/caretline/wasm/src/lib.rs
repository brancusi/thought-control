//! A tiny C-ABI wrapper around `caretline_next::Session`, for the site's playground.
//!
//! The page writes a state-protocol request (one JSON line, as `caretline serve` reads) into
//! memory from `cl_alloc`, calls `cl_handle`, and reads the response at `cl_out_ptr` for the
//! returned length. It is the same `Session::handle` that `caretline serve` answers, in
//! process: no I/O and no clock (the page sends `now_ms` when it wants time to pass).
//! Several sessions can live side by side, by number: the page keeps a live one and a
//! second one to replay traces into.
use caretline_next::{Session, State, Viewport};
use std::cell::RefCell;

thread_local! {
    static SESSIONS: RefCell<Vec<Session>> = const { RefCell::new(Vec::new()) };
    static OUT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Reserves `len` bytes for the page to write a request into.
#[no_mangle]
pub extern "C" fn cl_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Answers one request line (`len` bytes at `ptr`, freed here) in session `sid`, creating
/// sessions up to it as needed. Returns the response length.
///
/// # Safety
/// `ptr` must come from `cl_alloc(len)` and hold `len` initialised bytes.
#[no_mangle]
pub unsafe extern "C" fn cl_handle(sid: usize, ptr: *mut u8, len: usize) -> usize {
    let bytes = Vec::from_raw_parts(ptr, len, len.max(1));
    let line = String::from_utf8_lossy(&bytes).into_owned();
    let response = SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        while s.len() <= sid {
            s.push(Session::new(State::new("", None, Viewport { width: 60, height: 12 })));
        }
        s[sid].handle(&line, None).response
    });
    OUT.with(|o| {
        *o.borrow_mut() = response;
        o.borrow().len()
    })
}

/// Where the last response starts.
#[no_mangle]
pub extern "C" fn cl_out_ptr() -> *const u8 {
    OUT.with(|o| o.borrow().as_ptr())
}

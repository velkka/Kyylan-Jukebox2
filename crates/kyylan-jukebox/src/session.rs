//! Signing out and shutting down on Windows. A program with no console gets no signal for
//! either: Windows asks each top-level window instead, with `WM_QUERYENDSESSION` and then
//! `WM_ENDSESSION`, and ends the process soon after the last one returns. The installer's
//! Restart Manager asks the same way when it replaces a running copy. So the jukebox keeps a
//! hidden window of its own, on a thread of its own, just to hear that.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, MSG, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_OVERLAPPED,
};

/// The hidden window's class, which tests find it by.
pub const WINDOW_CLASS: &str = "KyylanJukeboxSession";

type OnEnd = Box<dyn Fn(&'static str) + Send + Sync>;
static ON_END: OnceLock<OnEnd> = OnceLock::new();

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Starts listening. `on_end` stops the jukebox; the process exits once it returns.
pub fn watch(on_end: impl Fn(&'static str) + Send + Sync + 'static) {
    if ON_END.set(Box::new(on_end)).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("session".into())
        .spawn(|| unsafe {
            let class = wide(WINDOW_CLASS);
            let instance = GetModuleHandleW(std::ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&definition);
            // A top-level window, never shown. A message-only window would be simpler, but
            // those aren't asked about the session ending.
            let window = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if window.is_null() {
                tracing::warn!("can't watch for signing out; the jukebox won't stop cleanly then");
                return;
            }
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        })
        .expect("starting the session thread");
}

fn end(reason: &'static str) -> ! {
    if let Some(on_end) = ON_END.get() {
        on_end(reason);
    }
    std::process::exit(0)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Never stand in the way of signing out.
        WM_QUERYENDSESSION => 1,
        // Stopping has to finish before returning: after that Windows may end the process
        // at any moment.
        WM_ENDSESSION if wparam != 0 => end("the Windows session is ending"),
        WM_ENDSESSION => 0,
        // `taskkill` without /F, or Task Scheduler ending the task.
        WM_CLOSE => end("asked to close"),
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

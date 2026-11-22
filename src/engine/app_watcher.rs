use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, bounded};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, GetWindowThreadProcessId, MSG,
    PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};
use windows::core::PWSTR;

static HOOK_SENDER: OnceLock<Sender<String>> = OnceLock::new();

pub struct ForegroundAppWatcher {
    rx: Receiver<String>,
    thread_id: u32,
    join_handle: Option<JoinHandle<()>>,
}

impl ForegroundAppWatcher {
    pub fn start() -> Self {
        let (tx, rx) = bounded::<String>(16);
        let _ = HOOK_SENDER.set(tx);

        let (ready_tx, ready_rx) = bounded::<u32>(1);
        let handle = thread::Builder::new()
            .name("ds-fg-watcher".to_owned())
            .spawn(move || {
                let tid = unsafe { GetCurrentThreadId() };
                let hook: HWINEVENTHOOK = unsafe {
                    SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    )
                };
                let _ = ready_tx.send(tid);
                let mut msg = MSG::default();
                unsafe {
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if !hook.is_invalid() {
                        let _ = UnhookWinEvent(hook);
                    }
                }
            })
            .ok();

        let thread_id = ready_rx.recv().unwrap_or(0);

        Self {
            rx,
            thread_id,
            join_handle: handle,
        }
    }

    #[inline]
    pub fn receiver(&self) -> &Receiver<String> {
        &self.rx
    }
}

impl Drop for ForegroundAppWatcher {
    fn drop(&mut self) {
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if hwnd.is_invalid() {
        return;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return;
    }
    let Ok(proc_handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return;
    };

    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            proc_handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    unsafe {
        let _ = CloseHandle(proc_handle);
    }
    if ok.is_err() || len == 0 {
        return;
    }

    let full = OsString::from_wide(&buf[..len as usize])
        .to_string_lossy()
        .into_owned();
    if let Some(exe) = Path::new(&full).file_name().and_then(|s| s.to_str()) {
        if let Some(tx) = HOOK_SENDER.get() {
            let _ = tx.try_send(exe.to_ascii_lowercase());
        }
    }
}

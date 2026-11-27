use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, bounded};
use parking_lot::Mutex;
use tracing::warn;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE,
    HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GWLP_USERDATA,
    GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HICON, ICON_BIG, ICON_SMALL,
    IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SW_RESTORE,
    SendMessageW, SetForegroundWindow, SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WM_SETICON, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

use crate::emu::EmulationMode;

const TRAY_CLASS_NAME: &str = "DSMasterTrayWindow";
const SINGLE_INSTANCE_MUTEX: &str = "Local\\DSMaster_SingleInstance_v1";
const TRAY_ICON_ID: u32 = 1;

const WM_TRAY_ICON: u32 = WM_APP + 1;
const WM_TRAY_UPDATE_TIP: u32 = WM_APP + 2;
const WM_TRAY_WAKE_EXISTING: u32 = WM_APP + 3;
const WM_TRAY_NOTIFY: u32 = WM_APP + 4;

const ID_TOGGLE_SHOW: usize = 1001;
const ID_EMU_NONE: usize = 1002;
const ID_EMU_X360: usize = 1003;
const ID_EMU_DS4: usize = 1004;
const ID_EMU_DS5: usize = 1007;
const ID_TOGGLE_CLOAK: usize = 1005;
const ID_QUIT: usize = 1006;

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

pub fn acquire_single_instance() -> Option<SingleInstanceGuard> {
    let name = to_wide_null(SINGLE_INSTANCE_MUTEX);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.ok()?;
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        wake_existing_instance();
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    Some(SingleInstanceGuard(handle))
}

fn wake_existing_instance() {
    let tray_class = to_wide_null(TRAY_CLASS_NAME);
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR(tray_class.as_ptr()), PCWSTR::null()) } {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_TRAY_WAKE_EXISTING, WPARAM(0), LPARAM(0));
        }
        return;
    }
    let title = to_wide_null("DSMaster");
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) } {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    ToggleVisibility,
    SetEmulation(EmulationMode),
    ToggleCloak,
    Quit,
}

#[derive(Debug, Clone)]
pub struct TrayState {
    pub window_visible: bool,
    pub emulation: EmulationMode,
    pub cloak_enabled: bool,
    pub hidhide_available: bool,
    pub connected_count: usize,
    pub active_profile: String,
}

impl TrayState {
    pub fn tooltip(&self) -> String {
        let mode = match self.emulation {
            EmulationMode::None => "Passthrough",
            EmulationMode::Xbox360 => "Xbox 360",
            EmulationMode::DualShock4 => "DualShock 4",
            EmulationMode::DualSense => "DualSense",
        };
        let raw = if self.connected_count == 0 {
            format!("DSMaster - {mode} (Idle)")
        } else if self.connected_count == 1 {
            format!("DSMaster - {mode} (1 connected, {})", self.active_profile)
        } else {
            format!(
                "DSMaster - {mode} ({} connected, {})",
                self.connected_count, self.active_profile
            )
        };
        let mut out = String::new();
        let mut utf16_len = 0usize;
        for ch in raw.chars() {
            let need = ch.len_utf16();
            if utf16_len + need > 127 {
                break;
            }
            out.push(ch);
            utf16_len += need;
        }
        out
    }
}

#[derive(Debug, Clone)]
struct PendingNotification {
    title: String,
    body: String,
    warning: bool,
}

struct TrayWindowCtx {
    tx: Sender<TrayAction>,
    state: Arc<Mutex<TrayState>>,
    pending_notifications: Arc<Mutex<Vec<PendingNotification>>>,
    hicon: HICON,
    taskbar_created_msg: u32,
}

pub struct SystemTray {
    msg_hwnd: Arc<AtomicIsize>,
    state: Arc<Mutex<TrayState>>,
    pending_notifications: Arc<Mutex<Vec<PendingNotification>>>,
    thread: Option<JoinHandle<()>>,
}

impl SystemTray {
    pub fn start(tx: Sender<TrayAction>, initial_state: TrayState) -> Option<Self> {
        let msg_hwnd = Arc::new(AtomicIsize::new(0));
        let state = Arc::new(Mutex::new(initial_state));
        let pending_notifications = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = bounded::<bool>(1);

        let hwnd_c = Arc::clone(&msg_hwnd);
        let state_c = Arc::clone(&state);
        let notif_c = Arc::clone(&pending_notifications);

        let thread = thread::Builder::new()
            .name("ds-tray".to_owned())
            .spawn(move || {
                run_tray_loop(tx, state_c, notif_c, hwnd_c, ready_tx);
            })
            .ok()?;

        match ready_rx.recv() {
            Ok(true) => Some(Self {
                msg_hwnd,
                state,
                pending_notifications,
                thread: Some(thread),
            }),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }

    pub fn update_state(&self, next: TrayState) {
        *self.state.lock() = next;
        let raw = self.msg_hwnd.load(Ordering::Acquire);
        if raw != 0 {
            let hwnd = HWND(raw as *mut core::ffi::c_void);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_TRAY_UPDATE_TIP, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn show_notification(&self, title: impl Into<String>, body: impl Into<String>, warning: bool) {
        {
            let mut q = self.pending_notifications.lock();
            if q.len() >= 8 {
                q.remove(0);
            }
            q.push(PendingNotification {
                title: title.into(),
                body: body.into(),
                warning,
            });
        }
        let raw = self.msg_hwnd.load(Ordering::Acquire);
        if raw != 0 {
            let hwnd = HWND(raw as *mut core::ffi::c_void);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_TRAY_NOTIFY, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        let raw = self.msg_hwnd.swap(0, Ordering::AcqRel);
        if raw != 0 {
            let hwnd = HWND(raw as *mut core::ffi::c_void);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn run_tray_loop(
    tx: Sender<TrayAction>,
    state: Arc<Mutex<TrayState>>,
    pending_notifications: Arc<Mutex<Vec<PendingNotification>>>,
    msg_hwnd: Arc<AtomicIsize>,
    ready_tx: Sender<bool>,
) {
    let hmod = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap_or_default();
    let hinstance = HINSTANCE(hmod.0);
    let class_name = to_wide_null(TRAY_CLASS_NAME);
    let window_title = to_wide_null("DSMasterTray");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(tray_wnd_proc),
        hInstance: hinstance,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };

    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
        let _ = ready_tx.send(false);
        return;
    }

    let hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(window_title.as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
    } {
        Ok(h) => h,
        Err(_) => {
            let _ = ready_tx.send(false);
            return;
        }
    };

    let Some(hicon) = create_tray_icon() else {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(false);
        return;
    };

    let tb_msg_name = to_wide_null("TaskbarCreated");
    let taskbar_created_msg = unsafe { RegisterWindowMessageW(PCWSTR(tb_msg_name.as_ptr())) };

    let ctx_box = Box::new(TrayWindowCtx {
        tx,
        state: Arc::clone(&state),
        pending_notifications,
        hicon,
        taskbar_created_msg,
    });
    let ctx_ptr = Box::into_raw(ctx_box);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
    }

    let initial_tip = state.lock().tooltip();
    if !add_tray_icon(hwnd, hicon, &initial_tip) {
        warn!("Shell_NotifyIconW(NIM_ADD) failed for system tray icon");
    }

    msg_hwnd.store(hwnd.0 as isize, Ordering::Release);
    let _ = ready_tx.send(true);

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    msg_hwnd.store(0, Ordering::Release);
    remove_tray_icon(hwnd);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let ctx = Box::from_raw(ctx_ptr);
        let _ = DestroyIcon(ctx.hicon);
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut TrayWindowCtx;
    if !ptr.is_null() {
        let ctx = unsafe { &*ptr };
        match msg {
            WM_TRAY_ICON => {
                let ev = (lparam.0 as u32) & 0xFFFF;
                if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                    let _ = ctx.tx.try_send(TrayAction::ShowWindow);
                } else if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                    show_context_menu(hwnd, ctx);
                }
                return LRESULT(0);
            }
            WM_TRAY_WAKE_EXISTING => {
                let _ = ctx.tx.try_send(TrayAction::ShowWindow);
                return LRESULT(0);
            }
            WM_TRAY_UPDATE_TIP => {
                let tip = ctx.state.lock().tooltip();
                update_tray_tip(hwnd, ctx.hicon, &tip);
                return LRESULT(0);
            }
            WM_TRAY_NOTIFY => {
                let batch: Vec<PendingNotification> = {
                    let mut q = ctx.pending_notifications.lock();
                    std::mem::take(&mut *q)
                };
                for item in batch {
                    send_tray_notification(hwnd, ctx.hicon, &item.title, &item.body, item.warning);
                }
                return LRESULT(0);
            }
            m if ctx.taskbar_created_msg != 0 && m == ctx.taskbar_created_msg => {
                let tip = ctx.state.lock().tooltip();
                let _ = add_tray_icon(hwnd, ctx.hicon, &tip);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    match msg {
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn fill_utf16<const N: usize>(buf: &mut [u16; N], text: &str) {
    buf.fill(0);
    for (idx, unit) in text.encode_utf16().take(N.saturating_sub(1)).enumerate() {
        buf[idx] = unit;
    }
}

fn fill_tip(buf: &mut [u16; 128], tip: &str) {
    fill_utf16(buf, tip);
}

fn send_tray_notification(hwnd: HWND, hicon: HICON, title: &str, body: &str, warning: bool) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_ICON | NIF_INFO,
        hIcon: hicon,
        dwInfoFlags: if warning { NIIF_WARNING } else { NIIF_INFO },
        ..Default::default()
    };
    fill_utf16(&mut nid.szInfoTitle, title);
    fill_utf16(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn add_tray_icon(hwnd: HWND, hicon: HICON, tip: &str) -> bool {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_ICON,
        hIcon: hicon,
        ..Default::default()
    };
    fill_tip(&mut nid.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool()
}

fn update_tray_tip(hwnd: HWND, hicon: HICON, tip: &str) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_ICON | NIF_TIP,
        hIcon: hicon,
        ..Default::default()
    };
    fill_tip(&mut nid.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn remove_tray_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn show_context_menu(hwnd: HWND, ctx: &TrayWindowCtx) {
    let snapshot = ctx.state.lock().clone();
    let Ok(hmenu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };

    let show_label = if snapshot.window_visible {
        to_wide_null("Hide to Tray")
    } else {
        to_wide_null("Show DSMaster")
    };
    let mode_none = to_wide_null("Mode: Off (Passthrough)");
    let mode_x360 = to_wide_null("Mode: Xbox 360");
    let mode_ds4 = to_wide_null("Mode: DualShock 4");
    let mode_ds5 = to_wide_null("Mode: DualSense");
    let cloak_label = to_wide_null("Cloak Physical Controller");
    let quit_label = to_wide_null("Quit DSMaster");

    let check_flag = |active: bool| if active { MF_CHECKED } else { MF_UNCHECKED };
    let cloak_flags = MF_STRING
        | check_flag(snapshot.cloak_enabled)
        | if snapshot.hidhide_available {
            MF_UNCHECKED
        } else {
            MF_GRAYED
        };

    unsafe {
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            ID_TOGGLE_SHOW,
            PCWSTR(show_label.as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | check_flag(snapshot.emulation == EmulationMode::None),
            ID_EMU_NONE,
            PCWSTR(mode_none.as_ptr()),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | check_flag(snapshot.emulation == EmulationMode::Xbox360),
            ID_EMU_X360,
            PCWSTR(mode_x360.as_ptr()),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | check_flag(snapshot.emulation == EmulationMode::DualShock4),
            ID_EMU_DS4,
            PCWSTR(mode_ds4.as_ptr()),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | check_flag(snapshot.emulation == EmulationMode::DualSense),
            ID_EMU_DS5,
            PCWSTR(mode_ds5.as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            hmenu,
            cloak_flags,
            ID_TOGGLE_CLOAK,
            PCWSTR(cloak_label.as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(hmenu, MF_STRING, ID_QUIT, PCWSTR(quit_label.as_ptr()));
    }

    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
    }

    let cmd = unsafe {
        TrackPopupMenu(
            hmenu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        )
    };

    unsafe {
        let _ = DestroyMenu(hmenu);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }

    let action = match cmd.0 as usize {
        ID_TOGGLE_SHOW => Some(TrayAction::ToggleVisibility),
        ID_EMU_NONE => Some(TrayAction::SetEmulation(EmulationMode::None)),
        ID_EMU_X360 => Some(TrayAction::SetEmulation(EmulationMode::Xbox360)),
        ID_EMU_DS4 => Some(TrayAction::SetEmulation(EmulationMode::DualShock4)),
        ID_EMU_DS5 => Some(TrayAction::SetEmulation(EmulationMode::DualSense)),
        ID_TOGGLE_CLOAK => Some(TrayAction::ToggleCloak),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    };
    if let Some(act) = action {
        let _ = ctx.tx.try_send(act);
    }
}

const APP_ICON_RESOURCE_ID: usize = 1;
const APP_ICON_BYTES: &[u8] = include_bytes!("../assets/icon.ico");

fn select_ico_frame(data: &[u8], desired_w: i32, desired_h: i32) -> Option<&[u8]> {
    if data.len() < 6 {
        return None;
    }
    let reserved = u16::from_le_bytes([data[0], data[1]]);
    let ico_type = u16::from_le_bytes([data[2], data[3]]);
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    if reserved != 0 || ico_type != 1 || count == 0 || data.len() < 6 + count * 16 {
        return None;
    }

    let target = desired_w.max(desired_h).max(16);
    let mut best_ge: Option<(i32, &[u8])> = None;
    let mut best_any: Option<(i32, &[u8])> = None;

    for idx in 0..count {
        let base = 6 + idx * 16;
        let w = if data[base] == 0 {
            256
        } else {
            data[base] as i32
        };
        let h = if data[base + 1] == 0 {
            256
        } else {
            data[base + 1] as i32
        };
        let size = u32::from_le_bytes([
            data[base + 8],
            data[base + 9],
            data[base + 10],
            data[base + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            data[base + 12],
            data[base + 13],
            data[base + 14],
            data[base + 15],
        ]) as usize;
        let Some(end) = offset.checked_add(size) else {
            continue;
        };
        if end > data.len() || size == 0 {
            continue;
        }
        let frame = &data[offset..end];
        let dim = w.min(h);
        if w == desired_w && h == desired_h {
            return Some(frame);
        }
        if dim >= target && best_ge.is_none_or(|(best_d, _)| dim < best_d) {
            best_ge = Some((dim, frame));
        }
        if best_any.is_none_or(|(best_d, _)| dim > best_d) {
            best_any = Some((dim, frame));
        }
    }

    best_ge.or(best_any).map(|(_, frame)| frame)
}

fn load_embedded_icon(width: i32, height: i32) -> Option<HICON> {
    let frame = select_ico_frame(APP_ICON_BYTES, width, height)?;
    unsafe {
        CreateIconFromResourceEx(frame, true, 0x0003_0000, width, height, LR_DEFAULTCOLOR).ok()
    }
}

fn load_app_icon(width: i32, height: i32, shared: bool) -> Option<HICON> {
    if let Ok(hmod) = unsafe { GetModuleHandleW(PCWSTR::null()) } {
        let hinstance = HINSTANCE(hmod.0);
        let flags = if shared {
            LR_DEFAULTCOLOR | LR_SHARED
        } else {
            LR_DEFAULTCOLOR
        };
        if let Ok(handle) = unsafe {
            LoadImageW(
                Some(hinstance),
                PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
                IMAGE_ICON,
                width,
                height,
                flags,
            )
        } {
            if !handle.is_invalid() {
                return Some(HICON(handle.0));
            }
        }
    }
    load_embedded_icon(width, height)
}

pub fn apply_window_icon(raw_hwnd: isize) {
    if raw_hwnd == 0 {
        return;
    }
    let hwnd = HWND(raw_hwnd as *mut core::ffi::c_void);
    let big_w = unsafe { GetSystemMetrics(SM_CXICON) }.max(32);
    let big_h = unsafe { GetSystemMetrics(SM_CYICON) }.max(32);
    if let Some(big) = load_app_icon(big_w, big_h, true) {
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(big.0 as isize)),
            );
        }
    }
    let small_w = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let small_h = unsafe { GetSystemMetrics(SM_CYSMICON) }.max(16);
    if let Some(small) = load_app_icon(small_w, small_h, true) {
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(small.0 as isize)),
            );
        }
    }
}

fn create_tray_icon() -> Option<HICON> {
    let w = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let h = unsafe { GetSystemMetrics(SM_CYSMICON) }.max(16);
    load_app_icon(w, h, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_stays_within_win32_limit() {
        let state = TrayState {
            window_visible: false,
            emulation: EmulationMode::Xbox360,
            cloak_enabled: true,
            hidhide_available: true,
            connected_count: 2,
            active_profile: "A".repeat(200),
        };
        let tip = state.tooltip();
        assert!(tip.encode_utf16().count() <= 127);
        assert!(tip.starts_with("DSMaster - Xbox 360"));
    }

    #[test]
    fn embedded_ico_selects_expected_frames() {
        assert!(select_ico_frame(APP_ICON_BYTES, 16, 16).is_some());
        assert!(select_ico_frame(APP_ICON_BYTES, 32, 32).is_some());
    }
}

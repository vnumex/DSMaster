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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    pub window_visible: bool,
    pub emulation: EmulationMode,
    pub cloak_enabled: bool,
    pub hidhide_available: bool,
    pub connected_count: usize,
    pub battery_percent: Option<u8>,
    pub battery_charging: bool,
}

impl TrayState {
    pub fn tooltip(&self) -> String {
        let mode = match self.emulation {
            EmulationMode::None => "Passthrough",
            EmulationMode::Xbox360 => "Xbox 360",
            EmulationMode::DualShock4 => "DualShock 4",
            EmulationMode::DualSense => "DualSense",
        };
        let batt_seg = self.battery_percent.map(|pct| {
            if self.battery_charging {
                format!(" · {pct}% Charging")
            } else {
                format!(" · {pct}%")
            }
        }).unwrap_or_default();
        let raw = if self.connected_count == 0 {
            format!("DSMaster - {mode} (Idle)")
        } else if self.connected_count == 1 {
            format!("DSMaster - {mode}{batt_seg}")
        } else {
            format!(
                "DSMaster - {mode}{batt_seg} ({} connected)",
                self.connected_count
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
    icon_key: Option<(Option<u8>, bool)>,
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

    let initial_icon_key = {
        let st = state.lock();
        if st.connected_count == 0 {
            (None, false)
        } else {
            (st.battery_percent, st.battery_charging)
        }
    };
    let Some(hicon) = create_battery_tray_icon(initial_icon_key.0, initial_icon_key.1)
        .or_else(create_tray_icon)
    else {
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
        icon_key: Some(initial_icon_key),
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
        let ctx = unsafe { &mut *ptr };
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
                let (tip, next_key) = {
                    let st = ctx.state.lock();
                    let key = if st.connected_count == 0 {
                        (None, false)
                    } else {
                        (st.battery_percent, st.battery_charging)
                    };
                    (st.tooltip(), key)
                };
                let mut old_icon = None;
                if ctx.icon_key != Some(next_key) {
                    if let Some(new_icon) = create_battery_tray_icon(next_key.0, next_key.1) {
                        old_icon = Some(ctx.hicon);
                        ctx.hicon = new_icon;
                        ctx.icon_key = Some(next_key);
                    }
                }
                update_tray_tip(hwnd, ctx.hicon, &tip);
                if let Some(prev) = old_icon {
                    unsafe {
                        let _ = DestroyIcon(prev);
                    }
                }
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

/// Renders a crisp 32-bit BGRA Windows ICO DIB for a horizontal battery tray icon.
///
/// - `battery_percent = None` renders a muted outline (disconnected / idle).
/// - `battery_percent = Some(pct)` fills proportionally with green (>40%), amber (21–40%),
///   or red (<=20%), and overlays a white lightning bolt when `charging` is true.
fn build_battery_ico_dib(
    width: usize,
    height: usize,
    battery_percent: Option<u8>,
    charging: bool,
) -> Vec<u8> {
    let w = width.clamp(16, 64);
    let h = height.clamp(16, 64);
    let xor_len = w * h * 4;
    let and_stride = w.div_ceil(32) * 4;
    let and_len = and_stride * h;
    let mut buf = vec![0u8; 40 + xor_len + and_len];

    // BITMAPINFOHEADER (40 bytes)
    buf[0..4].copy_from_slice(&40u32.to_le_bytes());
    buf[4..8].copy_from_slice(&(w as i32).to_le_bytes());
    buf[8..12].copy_from_slice(&((h * 2) as i32).to_le_bytes());
    buf[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
    buf[14..16].copy_from_slice(&32u16.to_le_bytes()); // 32-bit BGRA
    buf[20..24].copy_from_slice(&((xor_len + and_len) as u32).to_le_bytes());

    let set_px = |buf: &mut [u8], x: usize, y: usize, r: u8, g: u8, b: u8, a: u8| {
        if x >= w || y >= h {
            return;
        }
        // Bottom-up row order for DIB
        let dib_y = h - 1 - y;
        let off = 40 + (dib_y * w + x) * 4;
        buf[off] = b;
        buf[off + 1] = g;
        buf[off + 2] = r;
        buf[off + 3] = a;
    };

    // Scale coordinates cleanly from a 16x16 logical grid
    let sx = |v: f32| ((v / 16.0) * (w as f32)).round() as usize;
    let sy = |v: f32| ((v / 16.0) * (h as f32)).round() as usize;

    let x0 = sx(1.0);
    let x1 = sx(13.0).min(w - 2);
    let y0 = sy(4.0);
    let y1 = sy(12.0).min(h - 1);
    let border = (w / 16).max(1);

    let (or, og, ob, oa) = if battery_percent.is_some() {
        (235u8, 240u8, 248u8, 255u8)
    } else {
        (140u8, 148u8, 162u8, 210u8)
    };

    // Battery body outline (slightly rounded outer corners)
    for y in y0..=y1 {
        for x in x0..=x1 {
            let is_corner = (x == x0 || x == x1) && (y == y0 || y == y1);
            if is_corner {
                continue;
            }
            let on_edge = x < x0 + border
                || x + border > x1
                || y < y0 + border
                || y + border > y1;
            if on_edge {
                set_px(&mut buf, x, y, or, og, ob, oa);
            } else {
                // Dark translucent interior for contrast on both light and dark taskbars
                set_px(&mut buf, x, y, 20, 24, 30, 190);
            }
        }
    }

    // Positive battery terminal nub on the right
    let nub_x0 = (x1 + 1).min(w - 1);
    let nub_x1 = (x1 + border.max(1) + 1).min(w - 1);
    let nub_y0 = sy(6.0);
    let nub_y1 = sy(10.0).min(h - 1);
    for y in nub_y0..=nub_y1 {
        for x in nub_x0..=nub_x1 {
            set_px(&mut buf, x, y, or, og, ob, oa);
        }
    }

    // Fill level
    if let Some(pct) = battery_percent {
        let clamped = pct.min(100);
        let pad = border + (w / 16).max(1);
        let fx0 = x0 + pad;
        let fx1 = x1.saturating_sub(pad);
        let fy0 = y0 + pad;
        let fy1 = y1.saturating_sub(pad);
        if fx1 >= fx0 && fy1 >= fy0 {
            let max_w = fx1 - fx0 + 1;
            let fill_w = if clamped == 0 {
                0
            } else {
                (((max_w as f32) * (clamped as f32) / 100.0).round() as usize).clamp(1, max_w)
            };
            let (fr, fg, fb) = if charging {
                (52, 211, 153) // Emerald green
            } else if clamped > 40 {
                (74, 222, 128) // Crisp green
            } else if clamped > 20 {
                (251, 191, 36) // Warm amber
            } else {
                (248, 113, 113) // Alert red
            };
            for y in fy0..=fy1 {
                for x in fx0..(fx0 + fill_w) {
                    set_px(&mut buf, x, y, fr, fg, fb, 255);
                }
            }
        }
    }

    // Charging lightning bolt overlay in center of the battery
    if charging {
        let cx = (x0 + x1) as f32 * 0.5;
        let cy = (y0 + y1) as f32 * 0.5;
        let scale = (h as f32) / 16.0;
        let bolt_pts: [(f32, f32); 7] = [
            (0.8, -3.5),
            (-1.8, 0.2),
            (0.2, 0.2),
            (-0.8, 3.5),
            (1.8, -0.2),
            (-0.2, -0.2),
            (0.0, 0.0),
        ];
        for &(bx, by) in &bolt_pts {
            let px_x = (cx + bx * scale).round() as isize;
            let px_y = (cy + by * scale).round() as isize;
            for dy in 0..border.max(1) as isize {
                for dx in 0..border.max(1) as isize {
                    let tx = px_x + dx;
                    let ty = px_y + dy;
                    if tx >= 0 && ty >= 0 {
                        set_px(&mut buf, tx as usize, ty as usize, 255, 255, 255, 255);
                    }
                }
            }
        }
    }

    buf
}

fn create_battery_tray_icon(battery_percent: Option<u8>, charging: bool) -> Option<HICON> {
    let w = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let h = unsafe { GetSystemMetrics(SM_CYSMICON) }.clamp(16, 64);
    let dib = build_battery_ico_dib(w as usize, h as usize, battery_percent, charging);
    unsafe { CreateIconFromResourceEx(&dib, true, 0x0003_0000, w, h, LR_DEFAULTCOLOR).ok() }
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
            battery_percent: Some(65),
            battery_charging: true,
        };
        let tip = state.tooltip();
        assert!(tip.encode_utf16().count() <= 127);
        assert_eq!(tip, "DSMaster - Xbox 360 · 65% Charging (2 connected)");
    }

    #[test]
    fn embedded_ico_selects_expected_frames() {
        assert!(select_ico_frame(APP_ICON_BYTES, 16, 16).is_some());
        assert!(select_ico_frame(APP_ICON_BYTES, 32, 32).is_some());
    }

    #[test]
    fn battery_tray_dib_has_valid_header_and_pixels() {
        let dib = build_battery_ico_dib(32, 32, Some(75), true);
        let expected_len = 40 + (32 * 32 * 4) + (4 * 32);
        assert_eq!(dib.len(), expected_len);
        assert_eq!(u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]), 40);
        assert_eq!(i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]), 32);
        assert_eq!(i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]), 64);
        let non_zero_alpha = dib[40..40 + 32 * 32 * 4]
            .chunks_exact(4)
            .filter(|px| px[3] > 0)
            .count();
        assert!(non_zero_alpha > 100);
    }
}

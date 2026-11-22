use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, bounded};
use parking_lot::Mutex;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE,
    HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GWLP_USERDATA, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, HICON, ICONINFO, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SW_RESTORE, SetForegroundWindow, SetWindowLongPtrW, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{BOOL, PCWSTR};

use crate::emu::EmulationMode;

const TRAY_CLASS_NAME: &str = "DSMasterTrayWindow";
const SINGLE_INSTANCE_MUTEX: &str = "Local\\DSMaster_SingleInstance_v1";
const TRAY_ICON_ID: u32 = 1;

const WM_TRAY_ICON: u32 = WM_APP + 1;
const WM_TRAY_UPDATE_TIP: u32 = WM_APP + 2;
const WM_TRAY_WAKE_EXISTING: u32 = WM_APP + 3;

const ID_TOGGLE_SHOW: usize = 1001;
const ID_EMU_NONE: usize = 1002;
const ID_EMU_X360: usize = 1003;
const ID_EMU_DS4: usize = 1004;
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

struct TrayWindowCtx {
    tx: Sender<TrayAction>,
    state: Arc<Mutex<TrayState>>,
    hicon: HICON,
    taskbar_created_msg: u32,
}

pub struct SystemTray {
    msg_hwnd: Arc<AtomicIsize>,
    state: Arc<Mutex<TrayState>>,
    thread: Option<JoinHandle<()>>,
}

impl SystemTray {
    pub fn start(tx: Sender<TrayAction>, initial_state: TrayState) -> Option<Self> {
        let msg_hwnd = Arc::new(AtomicIsize::new(0));
        let state = Arc::new(Mutex::new(initial_state));
        let (ready_tx, ready_rx) = bounded::<bool>(1);

        let hwnd_c = Arc::clone(&msg_hwnd);
        let state_c = Arc::clone(&state);

        let thread = thread::Builder::new()
            .name("ds-tray".to_owned())
            .spawn(move || {
                run_tray_loop(tx, state_c, hwnd_c, ready_tx);
            })
            .ok()?;

        match ready_rx.recv() {
            Ok(true) => Some(Self {
                msg_hwnd,
                state,
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
        hicon,
        taskbar_created_msg,
    });
    let ctx_ptr = Box::into_raw(ctx_box);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
    }

    let initial_tip = state.lock().tooltip();
    let _ = add_tray_icon(hwnd, hicon, &initial_tip);

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

fn fill_tip(buf: &mut [u16; 128], tip: &str) {
    buf.fill(0);
    for (idx, unit) in tip.encode_utf16().take(127).enumerate() {
        buf[idx] = unit;
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
        ID_TOGGLE_CLOAK => Some(TrayAction::ToggleCloak),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    };
    if let Some(act) = action {
        let _ = ctx.tx.try_send(act);
    }
}

fn create_tray_icon() -> Option<HICON> {
    const W: usize = 32;
    const H: usize = 32;
    let mut pixels = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let idx = (y * W + x) * 4;
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let dist = (dx * dx + dy * dy).sqrt();

            let is_corner_cut = !(3..W - 3).contains(&x) && !(3..H - 3).contains(&y);
            if is_corner_cut {
                continue;
            }

            let is_border = x == 0 || x == W - 1 || y == 0 || y == H - 1;
            let is_ring = (7.2..=10.4).contains(&dist);
            let is_center = dist <= 3.8;

            let (r, g, b, a) = if is_border {
                (0x32, 0x38, 0x46, 0xFF)
            } else if is_ring {
                (0x3B, 0x82, 0xF6, 0xFF)
            } else if is_center {
                (0xE6, 0xE9, 0xEF, 0xFF)
            } else {
                (0x18, 0x1A, 0x20, 0xFF)
            };

            pixels[idx] = b;
            pixels[idx + 1] = g;
            pixels[idx + 2] = r;
            pixels[idx + 3] = a;
        }
    }

    let mask = [0u8; (W / 8) * H];
    let hbm_color =
        unsafe { CreateBitmap(W as i32, H as i32, 1, 32, Some(pixels.as_ptr().cast())) };
    let hbm_mask = unsafe { CreateBitmap(W as i32, H as i32, 1, 1, Some(mask.as_ptr().cast())) };
    if hbm_color.is_invalid() || hbm_mask.is_invalid() {
        unsafe {
            if !hbm_color.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm_color.0));
            }
            if !hbm_mask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
            }
        }
        return None;
    }

    let info = ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let hicon = unsafe { CreateIconIndirect(&info) }.ok();
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
    }
    hicon
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
}

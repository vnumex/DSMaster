#![cfg_attr(not(test), windows_subsystem = "windows")]

mod emu;
mod engine;
mod hid;
mod protocol;
mod ui;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::core::w;

use crate::emu::vigem::is_vigem_installed;
use crate::engine::{ControllerProfile, Engine};
use crate::ui::DsMasterView;

fn is_hidhide_available() -> bool {
    let handle = unsafe {
        CreateFileW(
            w!(r"\\.\HidHide"),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match handle {
        Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => {
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        _ => false,
    }
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let vigem_available = is_vigem_installed();
    let hidhide_available = is_hidhide_available();
    let initial_profile = ControllerProfile::default();
    let mut engine_opt = Some(Engine::start(initial_profile).map_err(|e| e.to_string()));

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(920.0), px(620.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(800.0), px(560.0))),
            titlebar: Some(TitlebarOptions {
                title: Some("DSMaster".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let engine_res = engine_opt
            .take()
            .unwrap_or_else(|| Err("Engine already initialized".to_owned()));

        let _ = cx.open_window(options, move |_, cx| {
            cx.new(|cx| {
                DsMasterView::new(
                    engine_res,
                    vigem_available,
                    hidhide_available,
                    initial_profile,
                    cx,
                )
            })
        });
        cx.activate(true);
    });
}

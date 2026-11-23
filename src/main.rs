#![cfg_attr(not(test), windows_subsystem = "windows")]

mod cloak;
mod emu;
mod engine;
mod hid;
mod protocol;
mod startup;
mod tray;
mod ui;

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::cloak::HidHideManager;
use crate::emu::vigem::is_vigem_installed;
use crate::engine::Engine;
use crate::engine::profiles_store::load_config;
use crate::ui::{DsMasterView, WindowControlState};

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let Some(_instance_guard) = tray::acquire_single_instance() else {
        return;
    };

    let vigem_available = is_vigem_installed();
    let hidhide_available = HidHideManager::is_available();
    let initial_config = load_config();
    let start_hidden = initial_config.work_in_background && initial_config.start_minimized;
    let mut engine_opt = Some(Engine::start(initial_config.clone()).map_err(|e| e.to_string()));

    Application::new().run(move |cx: &mut App| {
        let _ = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-400.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-500.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-600.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-700.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-400.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-600.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-700.ttf").as_slice()),
        ]);

        let bounds = Bounds::centered(None, size(px(1560.0), px(940.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1120.0), px(700.0))),
            show: !start_hidden,
            titlebar: Some(TitlebarOptions {
                title: Some("DSMaster".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let engine_res = engine_opt
            .take()
            .unwrap_or_else(|| Err("Engine already initialized".to_owned()));

        let _ = cx.open_window(options, move |window, cx| {
            let raw_hwnd = HasWindowHandle::window_handle(window)
                .ok()
                .and_then(|h| match h.as_raw() {
                    RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
                    _ => None,
                })
                .unwrap_or(0);

            tray::apply_window_icon(raw_hwnd);

            let ctl = WindowControlState {
                raw_hwnd,
                work_in_background: Arc::new(AtomicBool::new(initial_config.work_in_background)),
                window_hidden: Arc::new(AtomicBool::new(start_hidden)),
                force_quit: Arc::new(AtomicBool::new(false)),
            };
            let ctl_for_close = ctl.clone();

            let view = cx.new(|cx| {
                DsMasterView::new(
                    engine_res,
                    vigem_available,
                    hidhide_available,
                    initial_config,
                    ctl,
                    window,
                    cx,
                )
            });

            let view_for_close = view.clone();
            window.on_window_should_close(cx, move |_window, cx| {
                let bg = ctl_for_close.work_in_background.load(Ordering::Acquire);
                let quitting = ctl_for_close.force_quit.load(Ordering::Acquire);
                if bg && !quitting {
                    view_for_close.update(cx, |v, cx| v.hide_to_tray(cx));
                    false
                } else {
                    view_for_close.update(cx, |v, _| v.shutdown_all());
                    true
                }
            });

            view
        });

        if !start_hidden {
            cx.activate(true);
        }
    });
}

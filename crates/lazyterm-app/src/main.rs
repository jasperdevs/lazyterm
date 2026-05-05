#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use gpui::{
    px, size, App, AppContext, AssetSource, Bounds, SharedString, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
};
use gpui_platform::application;
use lazyterm_ui::LazytermApp;

struct Assets {
    base: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

fn main() {
    application()
        .with_assets(Assets {
            base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../logos"),
        })
        .run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1180.0), px(760.0)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Lazyterm".into()),
                            appears_transparent: true,
                            traffic_light_position: None,
                        }),
                        window_background: WindowBackgroundAppearance::Transparent,
                        window_decorations: Some(WindowDecorations::Client),
                        window_min_size: Some(size(px(820.0), px(520.0))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(LazytermApp::new),
                )
                .expect("open lazyterm window");

            window
                .update(cx, |app, window, cx| {
                    window.focus(&app.focus_handle(cx), cx);
                })
                .expect("focus lazyterm window");

            cx.activate(true);
        });
}

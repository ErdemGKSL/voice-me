//! `voice-me-app` — composition root.
//!
//! Story 1.2's slice: open a real window hosting `VoiceSetupView`, backed by
//! a real `FileSettingsStore`. No tray/hotkey wiring yet (Epic 2) — this
//! window shows only the Voice Setup screen directly.

use std::sync::Arc;

use gpui_kit::component::Root;
use gpui_kit::{AppContext as _, WindowOptions};
use voice_me_core::{FileSettingsStore, SettingsStore};
use voice_me_ui::VoiceSetupView;

fn main() {
    let settings_store: Arc<dyn SettingsStore> =
        Arc::new(FileSettingsStore::new().expect("failed to resolve settings/data directories"));

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), move |window, cx| {
                let view = cx.new(|_| VoiceSetupView::new(settings_store.clone()));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}

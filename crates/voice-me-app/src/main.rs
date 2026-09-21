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

    // Story 1.5: determine whether an active Reference Voice Sample already
    // exists before opening the window, so `VoiceSetupView` can decide
    // between the first-run empty-state copy and today's normal
    // recorder/replace view. A load failure is treated the same as "no
    // sample" — falling back to the empty-state copy is safe, whereas
    // silently treating an unreadable store as "has a sample" would hide
    // the first-run prompt from someone who actually needs it. The window
    // still opens unconditionally either way (Epic 2's tray/hotkey isn't
    // built yet).
    // Story 1.5 also restores the previously selected input device (if
    // any) from the same load, so the recorder starts on the device the
    // user last picked rather than always the OS default.
    let loaded_state = settings_store.load().ok();
    let has_active_sample = loaded_state
        .as_ref()
        .is_some_and(|state| state.reference_voice_sample.is_some());
    let selected_mic_device = loaded_state.and_then(|state| state.selected_mic_device);

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), move |window, cx| {
                let view = cx.new(|cx| {
                    VoiceSetupView::new(
                        settings_store.clone(),
                        has_active_sample,
                        selected_mic_device,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}

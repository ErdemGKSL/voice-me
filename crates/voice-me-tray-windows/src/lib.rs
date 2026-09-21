use voice_me_core::{AppEventSender, TrayPort, VoiceMeError};

/// Windows `TrayPort` adapter. Not yet implemented — Windows is deferred for
/// spec-2-1 (no Windows target/toolchain in this dev environment); see
/// `ARCHITECTURE-SPINE.md`'s Deferred section for the intended approach.
pub struct WindowsTrayAdapter;

impl TrayPort for WindowsTrayAdapter {
    fn show(&self, _cx: &mut gpui_kit::App, _events: AppEventSender) -> Result<(), VoiceMeError> {
        todo!()
    }
}

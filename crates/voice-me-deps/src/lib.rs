use voice_me_core::{DependencyProvisioningPort, VoiceMeError};

/// `DependencyProvisioningPort` adapter — detects/provisions runtime
/// dependencies from this repo's GitHub Releases. Not yet implemented —
/// see later stories.
pub struct DepsAdapter;

impl DependencyProvisioningPort for DepsAdapter {
    fn check(&self) -> Result<(), VoiceMeError> {
        todo!()
    }
}

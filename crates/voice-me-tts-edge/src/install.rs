//! The manual steps for a missing `edge-tts`, from `/etc/os-release`.

use voice_me_espeak::{distro_ids, is_opensuse, read_os_release};

/// The command that installs `pipx` on the system `os_release` (the
/// contents of `/etc/os-release`) describes, going by `ID` and then
/// `ID_LIKE` — the same detection as eSpeak NG's install command. `None`
/// for a distribution not known here.
pub fn pipx_command_for(os_release: &str) -> Option<&'static str> {
    distro_ids(os_release)
        .iter()
        .find_map(|id| match id.as_str() {
            "arch" => Some("sudo pacman -S python-pipx"),
            "debian" | "ubuntu" => Some("sudo apt install pipx"),
            "fedora" => Some("sudo dnf install pipx"),
            id if is_opensuse(id) => Some("sudo zypper install python3-pipx"),
            _ => None,
        })
}

/// What an unknown distribution is told instead of a pipx command.
pub const PIP_FALLBACK_STEP: &str = "Or: pip install --user edge-tts";

/// The last step: the Dependencies tab's own button.
pub const CHECK_AGAIN_STEP: &str = "Press Check again.";

/// The manual steps for a missing `edge-tts` on the system `os_release`
/// describes: the distribution's pipx command (or the pip fallback), then
/// "Press Check again.".
pub fn install_steps_for(os_release: &str) -> Vec<String> {
    let first = match pipx_command_for(os_release) {
        Some(command) => format!("If you don't have pipx: {command}"),
        None => PIP_FALLBACK_STEP.to_string(),
    };
    vec![first, CHECK_AGAIN_STEP.to_string()]
}

/// [`install_steps_for`] this machine.
pub fn install_steps() -> Vec<String> {
    install_steps_for(&read_os_release())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_distribution_gets_its_own_pipx_command() {
        for (os_release, command) in [
            (
                "NAME=\"Arch Linux\"\nID=arch\n",
                "sudo pacman -S python-pipx",
            ),
            (
                "ID=endeavouros\nID_LIKE=arch\n",
                "sudo pacman -S python-pipx",
            ),
            ("ID=debian\n", "sudo apt install pipx"),
            ("ID=ubuntu\nID_LIKE=debian\n", "sudo apt install pipx"),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n",
                "sudo apt install pipx",
            ),
            ("ID=\"fedora\"\n", "sudo dnf install pipx"),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                "sudo zypper install python3-pipx",
            ),
        ] {
            assert_eq!(pipx_command_for(os_release), Some(command), "{os_release}");
            let steps = install_steps_for(os_release);
            assert!(steps[0].ends_with(command), "{steps:?}");
            assert_eq!(steps[1], CHECK_AGAIN_STEP);
        }
    }

    #[test]
    fn an_unknown_distribution_gets_the_pip_fallback() {
        for os_release in ["ID=gentoo\n", ""] {
            assert_eq!(pipx_command_for(os_release), None);
            assert_eq!(
                install_steps_for(os_release),
                vec![PIP_FALLBACK_STEP.to_string(), CHECK_AGAIN_STEP.to_string()]
            );
        }
    }
}

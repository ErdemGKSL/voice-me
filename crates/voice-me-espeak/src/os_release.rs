//! The install command for `espeak-ng`, from `/etc/os-release`.

/// The package-manager command that installs `espeak-ng` on the system
/// `os_release` (the contents of `/etc/os-release`) describes, going by
/// `ID` and then `ID_LIKE`. `None` for a distribution not known here.
pub fn install_command_for(os_release: &str) -> Option<&'static str> {
    let field = |key: &str| {
        os_release.lines().find_map(|line| {
            let (name, value) = line.trim().split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches(['"', '\'']).to_string())
        })
    };
    let ids: Vec<String> = field("ID")
        .into_iter()
        .chain(field("ID_LIKE"))
        .flat_map(|value| {
            value
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();

    ids.iter().find_map(|id| match id.as_str() {
        "arch" => Some("sudo pacman -S espeak-ng"),
        "debian" | "ubuntu" => Some("sudo apt install espeak-ng"),
        "fedora" => Some("sudo dnf install espeak-ng"),
        id if id == "opensuse" || id.starts_with("opensuse-") => {
            Some("sudo zypper install espeak-ng")
        }
        _ => None,
    })
}

/// The one step that installs `espeak-ng` here: the distribution's command
/// when it is known, otherwise a sentence saying what to install.
pub fn install_step() -> String {
    let os_release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    match install_command_for(&os_release) {
        Some(command) => format!("Install it from a terminal: {command}"),
        None => GENERIC_INSTALL_STEP.to_string(),
    }
}

/// What an unknown distribution is told.
pub const GENERIC_INSTALL_STEP: &str =
    "Install the espeak-ng package with your distribution's package manager.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_distribution_gets_its_own_command() {
        assert_eq!(
            install_command_for("NAME=\"Arch Linux\"\nID=arch\n"),
            Some("sudo pacman -S espeak-ng")
        );
        assert_eq!(
            install_command_for("ID=debian\n"),
            Some("sudo apt install espeak-ng")
        );
        assert_eq!(
            install_command_for("ID=ubuntu\nID_LIKE=debian\n"),
            Some("sudo apt install espeak-ng")
        );
        assert_eq!(
            install_command_for("ID=\"fedora\"\n"),
            Some("sudo dnf install espeak-ng")
        );
        assert_eq!(
            install_command_for("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Some("sudo zypper install espeak-ng")
        );
    }

    #[test]
    fn a_derivative_falls_back_to_id_like() {
        assert_eq!(
            install_command_for("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some("sudo apt install espeak-ng")
        );
        assert_eq!(
            install_command_for("ID=endeavouros\nID_LIKE=arch\n"),
            Some("sudo pacman -S espeak-ng")
        );
    }

    #[test]
    fn an_unknown_or_missing_distribution_gets_no_command() {
        assert_eq!(install_command_for("ID=gentoo\n"), None);
        assert_eq!(install_command_for(""), None);
    }
}

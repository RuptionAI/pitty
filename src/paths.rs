//! Where Pitty keeps its files. Following the XDG Base Directory rules, a
//! relative XDG_CONFIG_HOME or XDG_DATA_HOME is ignored, and so is a relative
//! or empty HOME: Pitty changes into the project folder at startup, so a
//! relative path would put keys, settings, and history inside the project.
use std::{env, path::PathBuf};

fn absolute(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// `$XDG_CONFIG_HOME/pitty`, or `~/.config/pitty`.
pub fn config_dir() -> Result<PathBuf, String> {
    absolute("XDG_CONFIG_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".config")))
        .map(|base| base.join("pitty"))
        .ok_or_else(|| {
            "Cannot locate Pitty's settings: set HOME or XDG_CONFIG_HOME to an absolute path."
                .into()
        })
}

/// `$XDG_DATA_HOME/pitty`, or `~/.local/share/pitty`.
pub fn data_dir() -> Option<PathBuf> {
    absolute("XDG_DATA_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local/share")))
        .map(|base| base.join("pitty"))
}

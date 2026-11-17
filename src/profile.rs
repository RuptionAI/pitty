//! Local display labels. These values are never model instructions or credentials.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 16 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn default_name() -> String {
    "You".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default)]
    pub team: String,
    #[serde(skip)]
    config: PathBuf,
}

fn config_path() -> Result<PathBuf, String> {
    crate::paths::config_dir().map(|dir| dir.join("profile.json"))
}
fn label(value: &str, allow_empty: bool) -> Result<String, String> {
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    {
        return Err("Display labels cannot contain control characters, line breaks, or directional overrides.".into());
    }
    let value = value.trim();
    if (!allow_empty && value.is_empty()) || value.chars().count() > 40 {
        return Err(
            "Use a display label of 1–40 characters; /team off clears the team label.".into(),
        );
    }
    Ok(value.to_string())
}
fn open_config(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        const NONBLOCK: i32 = 0x4;
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        options.custom_flags(NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular profile file"));
    }
    Ok(file)
}
impl Profile {
    pub fn load() -> Result<Self, String> {
        Self::load_from(config_path()?)
    }
    pub fn load_from(config: PathBuf) -> Result<Self, String> {
        let metadata = match fs::metadata(&config) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    name: default_name(),
                    team: String::new(),
                    config,
                });
            }
            Err(error) => {
                return Err(format!(
                    "Cannot read display profile {}: {error}",
                    config.display()
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > LIMIT {
            return Err(format!(
                "Cannot read display profile {}: expected a regular JSON file no larger than 16 KiB.",
                config.display()
            ));
        }
        let mut bytes = Vec::new();
        open_config(&config)
            .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("Cannot read display profile {}: {e}", config.display()))?;
        if bytes.len() as u64 > LIMIT {
            return Err(format!(
                "Display profile {} exceeds 16 KiB.",
                config.display()
            ));
        }
        let mut profile: Self = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid display profile {}: {e}", config.display()))?;
        profile.name = label(&profile.name, false)
            .map_err(|e| format!("Invalid display profile {}: {e}", config.display()))?;
        profile.team = label(&profile.team, true)
            .map_err(|e| format!("Invalid display profile {}: {e}", config.display()))?;
        profile.config = config;
        Ok(profile)
    }
    pub fn set_name(&mut self, value: &str) -> Result<(), String> {
        let value = label(value, false)?;
        // Reload the other field so sequential edits from different sessions
        // do not silently restore an older team label.
        let mut next = Self::load_from(self.config.clone())?;
        next.name = value;
        next.persist()?;
        *self = next;
        Ok(())
    }
    pub fn set_team(&mut self, value: &str) -> Result<(), String> {
        let value = label(value, true)?;
        let mut next = Self::load_from(self.config.clone())?;
        next.team = if value.eq_ignore_ascii_case("off") {
            String::new()
        } else {
            value
        };
        next.persist()?;
        *self = next;
        Ok(())
    }
    fn persist(&self) -> Result<(), String> {
        let result = (|| -> io::Result<()> {
            let parent = self
                .config
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| io::Error::other("profile path needs a parent directory"))?;
            fs::create_dir_all(parent)?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(
                ".pitty-profile-{}-{now}-{sequence}.tmp",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            let write = (|| -> io::Result<()> {
                file.write_all(&serde_json::to_vec_pretty(self)?)?;
                file.sync_all()?;
                fs::rename(&temporary, &self.config)
            })();
            if write.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            write
        })();
        result.map_err(|e| format!("Cannot save display profile {}: {e}", self.config.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                    "pitty-profile-{}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                )))
        }
        fn path(&self) -> PathBuf {
            self.0.join("profile.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn defaults_persistence_and_sequential_sessions_preserve_other_field() {
        let fixture = Fixture::new();
        let mut first = Profile::load_from(fixture.path()).unwrap();
        let mut second = first.clone();
        assert_eq!(first.name, "You");
        assert!(first.team.is_empty());
        assert!(!fixture.path().exists());
        first.set_team("Research").unwrap();
        second.set_name("  Jeff  ").unwrap();
        assert_eq!(second.name, "Jeff");
        assert_eq!(second.team, "Research");
        first.set_team("off").unwrap();
        assert_eq!(first.name, "Jeff");
        assert!(first.team.is_empty());
        let body = fs::read_to_string(fixture.path()).unwrap();
        assert!(!body.contains("config"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn labels_are_unicode_bounded_and_invalid_updates_leave_profile_unchanged() {
        let fixture = Fixture::new();
        let mut p = Profile::load_from(fixture.path()).unwrap();
        p.set_name(&"界".repeat(40)).unwrap();
        let original = fs::read(fixture.path()).unwrap();
        for bad in [
            "x".repeat(41),
            "\n".into(),
            "hello\tworld".into(),
            "name\x1b[31m".into(),
            "\u{202e}name".into(),
            "a\u{2028}b".into(),
        ] {
            assert!(p.set_name(&bad).is_err());
            assert!(p.set_team(&bad).is_err());
            assert_eq!(fs::read(fixture.path()).unwrap(), original);
        }
        assert!(p.set_name("").is_err());
    }
    #[test]
    fn corrupt_oversized_and_nonregular_profiles_have_named_errors() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.0).unwrap();
        for bytes in [
            b"not JSON".to_vec(),
            vec![b' '; LIMIT as usize + 1],
            b"{\"name\":\"\\n\"}".to_vec(),
        ] {
            fs::write(fixture.path(), bytes).unwrap();
            let error = Profile::load_from(fixture.path()).unwrap_err();
            assert!(error.contains(fixture.path().to_str().unwrap()));
        }
        fs::remove_file(fixture.path()).unwrap();
        fs::create_dir(fixture.path()).unwrap();
        assert!(Profile::load_from(fixture.path()).is_err());
    }
}

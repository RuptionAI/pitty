//! Builds Pitty's embedded WebAssembly engine and native release binary.
//!
//! `engine` rebuilds crates/core for wasm32 and refreshes assets/pitty_core.wasm,
//! which the CLI embeds. `release` does that, then builds the release binary.
//! `install` does the host release, packages a fresh archive, installs the
//! binary, and checks the rendered site. Each archive goes in a new
//! subdirectory of `PITTY_ARCHIVE_DIR`, or of `dist` in the Cargo target
//! directory when that variable is unset. Both builds remap local paths so the
//! builder's home directory and dependency caches never end up inside the
//! shipped files.
use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, String>;

const USAGE: &str = "\
pitty-build: build Pitty from Rust

Usage: cargo run -p pitty-build -- COMMAND

Commands:
  engine   Rebuild the WebAssembly engine into assets/pitty_core.wasm
  release  Rebuild the engine, then the native release binary
  install  Release this host, package a fresh archive, install ~/.local/bin/pitty,
           and check website/

Environment:
  PITTY_ARCHIVE_DIR  Where install writes archives (default: TARGET_DIR/dist)";

fn root() -> PathBuf {
    // tools/build → repository root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn remap_flags(root: &Path) -> String {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cargo"));
    let rustup_home = env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".rustup"));
    let mut flags: Vec<String> = env::var("CARGO_ENCODED_RUSTFLAGS")
        .ok()
        .filter(|flags| !flags.is_empty())
        .map(|flags| flags.split('\x1f').map(str::to_owned).collect())
        .unwrap_or_default();
    for (from, to) in [
        (root.to_path_buf(), "."),
        (cargo_home, "/cargo"),
        (rustup_home, "/rustup"),
    ] {
        flags.push(format!("--remap-path-prefix={}={to}", from.display()));
    }
    flags.join("\x1f")
}

const RELEASE_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];
/// Chooses where `install` writes release archives.
const ARCHIVE_DIR_VAR: &str = "PITTY_ARCHIVE_DIR";

fn cargo(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let program = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = cargo_command(root, &program, args)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|error| format!("Cannot run {program}: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!("cargo {} failed.", args.join(" ")))
    }
}

fn cargo_inherit(root: &Path, args: &[&str]) -> Result<()> {
    let program = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = cargo_command(root, &program, args)
        .status()
        .map_err(|error| format!("Cannot run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed.", args.join(" ")))
    }
}

fn cargo_command(root: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env("CARGO_ENCODED_RUSTFLAGS", remap_flags(root))
        .args(args);
    if cfg!(target_os = "macos") {
        command.env("MACOSX_DEPLOYMENT_TARGET", "11.0");
    }
    command
}

fn target_directory(root: &Path) -> Result<PathBuf> {
    let metadata = cargo(root, &["metadata", "--format-version", "1", "--no-deps"])?;
    let metadata = String::from_utf8_lossy(&metadata);
    metadata
        .split("\"target_directory\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata did not report a target directory.".into())
}

fn engine(root: &Path) -> Result<()> {
    cargo(
        root,
        &[
            "build",
            "--locked",
            "-p",
            "pitty-core",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ],
    )
    .map_err(|error| {
        format!("{error} Install the target with `rustup target add wasm32-unknown-unknown`.")
    })?;
    let built = target_directory(root)?.join("wasm32-unknown-unknown/release/pitty_core.wasm");
    let bytes =
        fs::read(&built).map_err(|error| format!("Cannot read {}: {error}", built.display()))?;
    if !bytes.starts_with(b"\0asm") {
        return Err(format!("{} is not a WebAssembly module.", built.display()));
    }
    let destination = root.join("assets/pitty_core.wasm");
    fs::create_dir_all(root.join("assets")).map_err(|error| error.to_string())?;
    let changed = fs::read(&destination).map_or(true, |current| current != bytes);
    if changed {
        fs::write(&destination, &bytes)
            .map_err(|error| format!("Cannot write {}: {error}", destination.display()))?;
    }
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o644))
        .map_err(|error| error.to_string())?;
    println!(
        "assets/pitty_core.wasm: {} bytes ({}).",
        bytes.len(),
        if changed { "updated" } else { "unchanged" }
    );
    Ok(())
}

fn release(root: &Path) -> Result<PathBuf> {
    engine(root)?;
    cargo_inherit(root, &["build", "--locked", "--release"])?;
    let binary = target_directory(root)?.join("release/pitty");
    if !binary.is_file() {
        return Err(format!(
            "Release binary is missing at {}.",
            binary.display()
        ));
    }
    println!("Built the release binary in {}.", binary.display());
    Ok(binary)
}

fn compiler_host() -> Result<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| format!("Cannot run rustc: {error}"))?;
    if !output.status.success() {
        return Err("rustc -vV failed.".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_string)
        .ok_or_else(|| "rustc -vV did not report a host.".into())
}

fn workspace_version(root: &Path) -> Result<String> {
    let text = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("Cannot read Cargo.toml: {error}"))?;
    text.lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_string)
        })
        .filter(|version| {
            !version.is_empty()
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
        })
        .ok_or_else(|| "Cargo.toml has no package version.".into())
}

/// `PITTY_ARCHIVE_DIR` when it is set and not empty, otherwise `dist` in the
/// Cargo target directory.
fn archive_root(configured: Option<OsString>, target_dir: &Path) -> PathBuf {
    match configured {
        Some(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => target_dir.join("dist"),
    }
}

fn archive_directory(root: &Path, version: &str, target: &str) -> Result<PathBuf> {
    let base = archive_root(env::var_os(ARCHIVE_DIR_VAR), &target_directory(root)?);
    // pitty-release runs from the repository root, so a relative
    // PITTY_ARCHIVE_DIR is resolved here, against this command's directory.
    let base = std::path::absolute(&base)
        .map_err(|error| format!("Cannot resolve {}: {error}", base.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let directory = base.join(format!("{version}-{target}-{nanos}"));
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
    Ok(directory)
}

fn install_binary(binary: &Path) -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or("HOME is unset.")?;
    let directory = PathBuf::from(home).join(".local/bin");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
    let destination = directory.join("pitty");
    fs::copy(binary, &destination).map_err(|error| {
        format!(
            "Cannot install {} to {}: {error}",
            binary.display(),
            destination.display()
        )
    })?;
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).map_err(|error| {
        format!(
            "Cannot set permissions on {}: {error}",
            destination.display()
        )
    })?;
    Ok(destination)
}

fn source_is_dirty(root: &Path) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(true)
}

fn publish_note(version: &str, host: &str, dirty: bool) -> String {
    let others = RELEASE_TARGETS
        .iter()
        .copied()
        .filter(|target| *target != host)
        .collect::<Vec<_>>()
        .join(" and ");
    let source = if dirty {
        "This working tree has uncommitted changes, so that workflow would package the older commit until these changes are committed and pushed."
    } else {
        "It builds the revision that is on GitHub."
    };
    format!(
        "\
This machine packages {host} only. A local cargo build --target for {others} is not the release path this repo uses. The Release packages workflow runs those hosts. {source} It stores the three archives as artifacts for 14 days and does not publish pitty.tech.
Upload the three pitty-{version}-*.tar.gz files and their .sha256 files into /downloads/ first, then the contents of website/."
    )
}

fn install(root: &Path) -> Result<()> {
    let host = compiler_host()?;
    if !RELEASE_TARGETS.contains(&host.as_str()) {
        return Err(format!(
            "This host is {host}. `install` packages {}.",
            RELEASE_TARGETS.join(", ")
        ));
    }
    let binary = release(root)?;
    let version = workspace_version(root)?;
    let output = archive_directory(root, &version, &host)?;
    let output_arg = output.to_string_lossy().into_owned();
    cargo_inherit(
        root,
        &[
            "run",
            "--locked",
            "-p",
            "pitty-release",
            "--",
            "--target",
            &host,
            "--output",
            &output_arg,
        ],
    )?;
    let installed = install_binary(&binary)?;
    cargo_inherit(
        root,
        &[
            "run",
            "--manifest-path",
            "site/Cargo.toml",
            "--locked",
            "-p",
            "pitty-site",
            "--",
            "pages",
        ],
    )?;
    cargo_inherit(
        root,
        &[
            "run",
            "--manifest-path",
            "site/Cargo.toml",
            "--locked",
            "-p",
            "pitty-site",
            "--",
            "check",
        ],
    )?;
    let name = format!("pitty-{version}-{host}.tar.gz");
    println!("Installed {}.", installed.display());
    println!("Archive: {}", output.join(&name).display());
    println!("{}", publish_note(&version, &host, source_is_dirty(root)));
    Ok(())
}

fn main() -> ExitCode {
    let root = root();
    let result = match env::args().nth(1).as_deref() {
        Some("engine") => engine(&root),
        Some("release") => release(&root).map(|_| ()),
        Some("install") => install(&root),
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("Unknown command `{other}`.\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pitty-build: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remapping_covers_repository_and_toolchain_paths() {
        let flags = remap_flags(Path::new("/work/pitty"));
        assert!(flags.contains("--remap-path-prefix=/work/pitty=."));
        assert!(flags.contains("=/cargo"));
        assert!(flags.contains("=/rustup"));
    }
    #[test]
    fn root_is_the_repository() {
        assert!(root().join("Cargo.toml").is_file());
        assert!(root().join("crates/core").is_dir());
    }
    #[test]
    fn version_comes_from_the_workspace_manifest() {
        assert_eq!(workspace_version(&root()).unwrap(), "0.8.0");
    }
    #[test]
    fn archives_go_to_the_configured_directory_or_target_dist() {
        let target = Path::new("/work/target");
        assert_eq!(archive_root(None, target), Path::new("/work/target/dist"));
        // An empty value counts as unset rather than the current directory.
        assert_eq!(
            archive_root(Some(OsString::new()), target),
            Path::new("/work/target/dist")
        );
        assert_eq!(
            archive_root(Some(OsString::from("/archives/pitty")), target),
            Path::new("/archives/pitty")
        );
    }
    #[test]
    fn publish_note_names_the_other_hosts_and_the_upload_order() {
        let note = publish_note("0.7.0", "aarch64-apple-darwin", true);
        assert!(note.contains("pitty-0.7.0-"));
        assert!(note.contains("x86_64-apple-darwin"));
        assert!(note.contains("x86_64-unknown-linux-gnu"));
        assert!(note.contains("uncommitted changes"));
        assert!(note.contains("/downloads/"));
        assert!(note.contains("website/"));
        assert!(!note.contains("cargo build --target x86_64-unknown-linux-gnu"));
    }
}

//! Packages an already-built native binary. Never builds Pitty or publishes assets.
use serde_json::Value;
use std::{
    error::Error,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
fn json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn text<'a>(object: &'a Value, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Missing metadata field: {key}").into())
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}
fn files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            return Err("Unexpected symlink in package notice sources".into());
        }
        if entry.file_type()?.is_dir() {
            found.extend(files(&entry.path())?);
        } else if entry.file_type()?.is_file() {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}
fn copy(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(
        destination
            .parent()
            .ok_or("Missing destination directory")?,
    )?;
    fs::copy(source, destination)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o644))?;
    Ok(())
}
fn write(path: &Path, value: &str) -> Result<()> {
    fs::write(path, value)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    Ok(())
}
fn notices(metadata: &Value, stage: &Path) -> Result<usize> {
    let fallback = json(&root().join("tools/release/license-fallbacks.json"))?;
    let mut packages: Vec<&Value> = metadata["packages"]
        .as_array()
        .ok_or("Missing packages")?
        .iter()
        .filter(|p| !p["source"].is_null())
        .collect();
    packages.sort_by_key(|p| (p["name"].as_str(), p["version"].as_str()));
    let mut inventory = String::from(
        "Pitty third-party notices\n\nThis inventory conservatively includes resolved native dependencies, build tools, and Rust standard-library notices. Some listed components may not be linked into this binary. The embedded WebAssembly engine uses the same listed serde dependencies.\n\n",
    );
    for package in &packages {
        let name = text(package, "name")?;
        let identity = format!("{name}-{}", text(package, "version")?);
        if !safe_name(&identity) {
            return Err("Unsafe dependency name".into());
        }
        let manifest = Path::new(text(package, "manifest_path")?);
        let source = manifest
            .parent()
            .ok_or("Missing dependency directory")?
            .canonicalize()?;
        let destination = stage.join("licenses").join(&identity);
        fs::create_dir_all(&destination)?;
        let mut paths: Vec<PathBuf> = files(&source)?
            .into_iter()
            .filter(|p| {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                ["license", "copying", "copyright", "notice", "unlicense"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
            })
            .collect();
        if let Some(license) = package["license_file"].as_str() {
            paths.push(source.join(license));
        }
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            let record = &fallback[&identity];
            let vcs = json(&source.join(".cargo_vcs_info.json"))?;
            if record.is_null() || text(record, "commit")? != text(&vcs["git"], "sha1")? {
                return Err(format!("Missing reviewed license notices for {identity}").into());
            }
            let mut origins = String::new();
            for item in record["files"]
                .as_array()
                .ok_or("Missing fallback license files")?
            {
                let filename = text(item, "name")?;
                if !safe_name(filename) {
                    return Err("Unsafe license filename".into());
                }
                write(&destination.join(filename), text(item, "text")?)?;
                origins.push_str(text(item, "url")?);
                origins.push('\n');
            }
            write(&destination.join("SOURCES.txt"), &origins)?;
        } else {
            for path in paths {
                let path = path.canonicalize()?;
                let relative = path.strip_prefix(&source)?;
                if fs::metadata(&path)?.len() > 4 * 1024 * 1024 {
                    return Err(format!("Unexpectedly large license notice in {identity}").into());
                }
                copy(&path, &destination.join(relative))?;
            }
        }
        let license = package["license"]
            .as_str()
            .unwrap_or("See supplied license");
        let repository = package["repository"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("https://crates.io/crates/{name}"));
        inventory.push_str(&format!(
            "{identity}: {license}\n  Source: {repository}\n  Notices: licenses/{identity}/\n\n"
        ));
    }
    let rust = PathBuf::from(run("rustc", &["--print", "sysroot"])?).join("share/doc/rust");
    let copyright = if rust.join("COPYRIGHT-library.html").is_file() {
        rust.join("COPYRIGHT-library.html")
    } else {
        rust.join("COPYRIGHT.html")
    };
    if !copyright.is_file() || !rust.join("licenses").is_dir() {
        return Err("Rust license notices are missing; install its rust-docs component".into());
    }
    let destination = stage.join("licenses/rust");
    copy(
        &copyright,
        &destination.join(copyright.file_name().unwrap()),
    )?;
    for path in files(&rust.join("licenses"))? {
        copy(&path, &destination.join(path.strip_prefix(&rust)?))?;
    }
    inventory.push_str(&format!(
        "Rust: {}\n  Notices: licenses/rust/\n",
        run("rustc", &["--version"])?
    ));
    write(&stage.join("THIRD-PARTY-NOTICES.txt"), &inventory)?;
    Ok(packages.len())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn main() {
    if let Err(error) = package() {
        eprintln!("Packaging failed: {error}");
        std::process::exit(1);
    }
}
fn package() -> Result<()> {
    let mut target = None;
    let mut output = None;
    let mut binary = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--help" {
            println!(
                "pitty-release --target TARGET --output DIRECTORY [--binary PATH]\nTargets: {}",
                TARGETS.join(", ")
            );
            return Ok(());
        }
        let value = args.next().ok_or("Option requires a value")?;
        match argument.as_str() {
            "--target" => target = Some(value),
            "--output" => output = Some(PathBuf::from(value)),
            "--binary" => binary = Some(PathBuf::from(value)),
            _ => return Err(format!("Unknown option: {argument}").into()),
        }
    }
    let target = target.ok_or("--target is required")?;
    let output = output.ok_or("--output is required")?;
    if !TARGETS.contains(&target.as_str()) {
        return Err("Unsupported target".into());
    }
    let compiler = run("rustc", &["-vV"])?;
    let host = compiler
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .ok_or("Missing Rust host")?;
    if host != target {
        return Err(format!("Native packaging requires host {target}; got {host}").into());
    }
    let metadata: Value = serde_json::from_str(&run(
        "cargo",
        &[
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            &target,
        ],
    )?)?;
    let package = metadata["packages"]
        .as_array()
        .ok_or("Missing packages")?
        .iter()
        .find(|p| p["name"] == "pitty" && p["source"].is_null())
        .ok_or("Missing Pitty package")?;
    let version = text(package, "version")?;
    if !safe_name(version) {
        return Err("Unsafe release version".into());
    }
    let binary = binary
        .unwrap_or(PathBuf::from(text(&metadata, "target_directory")?).join("release/pitty"))
        .canonicalize()?;
    let binary_str = binary.to_str().ok_or("Binary path must be UTF-8")?;
    let description = run("file", &["-b", binary_str])?;
    let architecture = match target.as_str() {
        "aarch64-apple-darwin" => "arm64",
        "x86_64-apple-darwin" => "x86_64",
        _ => "x86-64",
    };
    if !description.contains(architecture) {
        return Err("Binary architecture does not match target".into());
    }
    if run(binary_str, &["--version"])? != format!("pitty {version}") {
        return Err("Binary version does not match Cargo.toml; rebuild before packaging".into());
    }
    let source_state = if run(
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    )?
    .is_empty()
    {
        "clean"
    } else {
        "uncommitted changes; candidate only"
    };
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let name = format!("pitty-{version}-{target}");
    let archive = output.join(format!("{name}.tar.gz"));
    let checksum = output.join(format!("{name}.tar.gz.sha256"));
    if archive.exists() || checksum.exists() {
        return Err("Release output already exists; use a fresh directory".into());
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary_path = output.join(format!(".pitty-package-{}-{nonce}", std::process::id()));
    fs::create_dir(&temporary_path)?;
    fs::set_permissions(&temporary_path, fs::Permissions::from_mode(0o700))?;
    let temporary = Temporary(temporary_path);
    let stage = temporary.0.join(&name);
    fs::create_dir(&stage)?;
    copy(&binary, &stage.join("pitty"))?;
    fs::set_permissions(stage.join("pitty"), fs::Permissions::from_mode(0o755))?;
    for filename in ["LICENSE", "INSTALL.md"] {
        copy(&root().join(filename), &stage.join(filename))?;
    }
    let count = notices(&metadata, &stage)?;
    write(
        &stage.join("BUILD.txt"),
        &format!(
            "Version: {version}\nTarget: {target}\nCommit: {}\nSource state: {source_state}\nCompiler: {}\n",
            run("git", &["rev-parse", "HEAD"])?,
            run("rustc", &["--version"])?
        ),
    )?;
    let temporary_archive = temporary.0.join(format!("{name}.tar.gz"));
    let mut tar = Command::new("tar");
    tar.env("COPYFILE_DISABLE", "1");
    if cfg!(target_os = "macos") {
        tar.args([
            "--uid", "0", "--gid", "0", "--uname", "root", "--gname", "root",
        ]);
    } else {
        tar.args(["--owner=0", "--group=0", "--numeric-owner"]);
    }
    let result = tar
        .arg("-czf")
        .arg(&temporary_archive)
        .arg("-C")
        .arg(&temporary.0)
        .arg(&name)
        .output()?;
    if !result.status.success() {
        return Err(format!("tar failed: {}", String::from_utf8_lossy(&result.stderr)).into());
    }
    let archive_str = temporary_archive
        .to_str()
        .ok_or("Archive path must be UTF-8")?;
    let digest_output = if cfg!(target_os = "macos") {
        run("shasum", &["-a", "256", archive_str])?
    } else {
        run("sha256sum", &[archive_str])?
    };
    let digest = digest_output
        .split_whitespace()
        .next()
        .ok_or("Missing checksum")?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid checksum output".into());
    }
    // Both paths are on the output filesystem; hard_link fails if another packager
    // created the destination after our initial existence check.
    fs::hard_link(&temporary_archive, &archive)?;
    write(&checksum, &format!("{digest}  {name}.tar.gz\n"))?;
    println!(
        "Packaged {name}.tar.gz with {count} dependency notice sets and Rust notices\n{name}.tar.gz.sha256"
    );
    Ok(())
}

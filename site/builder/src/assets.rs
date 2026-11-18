//! Fingerprinted static assets and the browser WebAssembly bundle.
use crate::{Result, output::Tree};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Paths of fingerprinted assets, as referenced from pages.
pub struct Site {
    pub css: String,
    pub favicon: String,
    pub touch_icon: String,
    pub social_image: String,
    pub browser_js: String,
    pub browser_wasm: String,
    pub notices: String,
    pub ruption_mark: String,
    /// The demo video on /demo/ and the still shown before it plays.
    pub demo_video: String,
    pub demo_poster: String,
    /// Font files the stylesheet uses, preloaded on every page.
    pub fonts: Vec<String>,
}

/// The browser crate compiled to WebAssembly plus its generated binding module.
pub struct BrowserBundle {
    js: String,
    wasm: Vec<u8>,
    wasm_name: String,
}

/// The browser crate's output name. Its files are `browser.HASH.js` and
/// `browser.HASH.wasm`.
const BUNDLE: &str = "browser";
const INIT: &str = "\n__wbg_init();\n";
const EXPORTS: &str = "export { initSync, __wbg_init as default };";

/// Self-hosted typefaces (SIL Open Font License 1.1). styles.css names each
/// as `url("fonts/NAME.woff2")`, which the build replaces with its
/// fingerprinted path.
const FONTS: &[&str] = &["atkinson-hyperlegible-next", "atkinson-hyperlegible-mono"];

pub fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))
}

fn read_text(path: &Path) -> Result<String> {
    String::from_utf8(read(path)?).map_err(|_| format!("{} is not UTF-8", path.display()))
}

fn run(command: &mut Command, what: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .map_err(|error| format!("Cannot start {what}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{what} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

/// Keep the builder's home directory and dependency-cache paths out of the Wasm.
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

pub fn build_browser_bundle(root: &Path) -> Result<BrowserBundle> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = root.join("site/Cargo.toml");
    println!("Compiling the browser crate to WebAssembly…");
    run(
        Command::new(&cargo)
            .current_dir(root)
            .env("CARGO_ENCODED_RUSTFLAGS", remap_flags(root))
            .arg("build")
            .arg("--manifest-path")
            .arg(&manifest)
            .args([
                "-p",
                "pitty-site-browser",
                "--release",
                "--locked",
                "--target",
                "wasm32-unknown-unknown",
            ]),
        "cargo build for wasm32-unknown-unknown (install it with `rustup target add wasm32-unknown-unknown`)",
    )?;
    let metadata = run(
        Command::new(&cargo)
            .current_dir(root)
            .arg("metadata")
            .arg("--manifest-path")
            .arg(&manifest)
            .args(["--format-version", "1", "--no-deps"]),
        "cargo metadata",
    )?;
    let metadata = String::from_utf8_lossy(&metadata);
    let target = metadata
        .split("\"target_directory\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or("cargo metadata did not report a target directory")?;
    let input = Path::new(target).join("wasm32-unknown-unknown/release/pitty_site_browser.wasm");
    let mut output = wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&input)
        .out_name(BUNDLE)
        .web(true)
        .map_err(|error| error.to_string())?
        .typescript(false)
        // The library omits the default Wasm URL unless asked; the CLI includes it.
        .omit_default_module_path(false)
        .generate_output()
        .map_err(|error| format!("wasm-bindgen failed: {error}"))?;
    let wasm = output.wasm_mut().emit_wasm();
    let wasm_name = format!("{BUNDLE}.{}.wasm", fingerprint(&wasm));
    let glue = output.js();
    let default_url = format!("{BUNDLE}_bg.wasm");
    let references = glue.matches(&default_url).count();
    if references != 1 || !glue.trim_end().ends_with(EXPORTS) {
        let tail: String = glue
            .trim_end()
            .chars()
            .rev()
            .take(160)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!(
            "wasm-bindgen output changed shape ({references} Wasm URL references; ends with {tail:?}). Review the bundle step in site/builder/src/assets.rs."
        ));
    }
    // The generated module starts itself, so pages need no separate loader.
    let js = format!("{}{INIT}", glue.replace(&default_url, &wasm_name));
    Ok(BrowserBundle {
        js,
        wasm,
        wasm_name,
    })
}

/// Reuse the committed bundle so pages can be rendered without a Wasm toolchain.
pub fn existing_browser_bundle(website: &Path) -> Result<BrowserBundle> {
    let directory = website.join("assets");
    let mut scripts = Vec::new();
    let mut modules = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| format!("Cannot read {}: {error}", directory.display()))?
    {
        let name = entry
            .map_err(|error| error.to_string())?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let parts: Vec<&str> = name.split('.').collect();
        if let [stem, hash, extension] = parts.as_slice()
            && *stem == BUNDLE
            && hash.len() == 12
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            match *extension {
                "js" => scripts.push(name),
                "wasm" => modules.push(name),
                _ => {}
            }
        }
    }
    let ([script], [module]) = (scripts.as_slice(), modules.as_slice()) else {
        return Err(format!(
            "website/assets/ must hold exactly one {BUNDLE}.*.js and {BUNDLE}.*.wasm. Run the `build` command to compile them."
        ));
    };
    let js = String::from_utf8(read(&directory.join(script))?)
        .map_err(|_| format!("{script} is not UTF-8"))?;
    let wasm = read(&directory.join(module))?;
    if *module != format!("{BUNDLE}.{}.wasm", fingerprint(&wasm))
        || *script != format!("{BUNDLE}.{}.js", fingerprint(js.as_bytes()))
    {
        return Err(
            "A browser asset's contents do not match its fingerprint. Run the `build` command."
                .into(),
        );
    }
    if !js.ends_with(INIT) || !js.contains(module.as_str()) {
        return Err(
            "The committed browser bundle predates the current format. Run the `build` command."
                .into(),
        );
    }
    Ok(BrowserBundle {
        js,
        wasm,
        wasm_name: module.clone(),
    })
}

fn add(tree: &mut Tree, stem: &str, extension: &str, bytes: Vec<u8>) -> Result<String> {
    let path = format!("assets/{stem}.{}.{extension}", fingerprint(&bytes));
    tree.insert(&path, bytes)?;
    Ok(format!("/{path}"))
}

/// Point each `url("fonts/NAME.woff2")` in the stylesheet at its fingerprinted
/// file. Every listed font must be used, and nothing else may be referenced.
fn link_fonts(css: &str, fonts: &[(&str, String)]) -> Result<String> {
    let mut linked = css.to_owned();
    for (name, url) in fonts {
        let placeholder = format!("url(\"fonts/{name}.woff2\")");
        if !linked.contains(&placeholder) {
            return Err(format!("styles.css does not use the font {name}."));
        }
        linked = linked.replace(&placeholder, &format!("url(\"{url}\")"));
    }
    if linked.contains("url(\"fonts/") {
        return Err("styles.css names a font file that site/static/fonts/ does not list.".into());
    }
    Ok(linked)
}

/// One plain-text file covering everything third parties wrote that the site
/// serves: the typefaces and the Rust crates inside the browser WebAssembly.
fn notices(root: &Path) -> Result<Vec<u8>> {
    let mut text = String::from(
        "pitty.tech: open-source notices\n\n\
         The site serves two typefaces under the SIL Open Font License 1.1 and\n\
         a WebAssembly module compiled from Rust crates. Their notices follow.\n\n",
    );
    for name in FONTS {
        text.push_str(&"=".repeat(72));
        text.push('\n');
        text.push_str(&read_text(
            &root.join(format!("site/static/fonts/OFL-{name}.txt")),
        )?);
        text.push_str("\n\n");
    }
    text.push_str(&"=".repeat(72));
    text.push('\n');
    text.push_str(&read_text(
        &root.join("site/browser/THIRD-PARTY-NOTICES.txt"),
    )?);
    Ok(text.into_bytes())
}

pub fn collect(root: &Path, browser: BrowserBundle, tree: &mut Tree) -> Result<Site> {
    let statics = root.join("site/static");
    let wasm_path = format!("assets/{}", browser.wasm_name);
    tree.insert(&wasm_path, browser.wasm)?;
    let mut fonts = Vec::new();
    for name in FONTS {
        let bytes = read(&statics.join(format!("fonts/{name}.woff2")))?;
        fonts.push((*name, add(tree, name, "woff2", bytes)?));
    }
    let css = link_fonts(&read_text(&statics.join("styles.css"))?, &fonts)?;
    Ok(Site {
        css: add(tree, "styles", "css", css.into_bytes())?,
        favicon: add(tree, "favicon", "svg", read(&statics.join("favicon.svg"))?)?,
        touch_icon: add(
            tree,
            "touch-icon",
            "png",
            read(&statics.join("touch-icon.png"))?,
        )?,
        social_image: add(tree, "social", "png", read(&statics.join("social.png"))?)?,
        notices: add(tree, "notices", "txt", notices(root)?)?,
        ruption_mark: add(
            tree,
            "ruption-mark",
            "svg",
            read(&statics.join("ruption-mark.svg"))?,
        )?,
        browser_js: add(tree, BUNDLE, "js", browser.js.into_bytes())?,
        browser_wasm: format!("/{wasm_path}"),
        demo_video: add(
            tree,
            "pitty-demo",
            "mp4",
            read(&statics.join("media/pitty-demo.mp4"))?,
        )?,
        demo_poster: add(
            tree,
            "pitty-demo-poster",
            "png",
            read(&statics.join("media/pitty-demo-poster.png"))?,
        )?,
        fonts: fonts.into_iter().map(|(_, url)| url).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprints_are_twelve_hex_digits_of_sha256() {
        assert_eq!(fingerprint(b"pitty"), "b3f13115b3d7");
        assert_eq!(fingerprint(b"").len(), 12);
    }
    #[test]
    fn fonts_link_to_fingerprinted_files_and_nothing_else() {
        let fonts = [("a", "/assets/a.0123456789ab.woff2".to_owned())];
        let css = "@font-face{src:url(\"fonts/a.woff2\") format(\"woff2\")}";
        assert_eq!(
            link_fonts(css, &fonts).unwrap(),
            "@font-face{src:url(\"/assets/a.0123456789ab.woff2\") format(\"woff2\")}"
        );
        assert!(link_fonts("body{}", &fonts).is_err());
        let stray = format!("{css}@font-face{{src:url(\"fonts/b.woff2\")}}");
        assert!(link_fonts(&stray, &fonts).is_err());
    }
}

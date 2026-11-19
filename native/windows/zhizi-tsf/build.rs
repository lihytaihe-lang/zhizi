use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

const RC_TEMPLATE_REL: &str = "resources/zhizi.rc";
const ICON_FILE_NAME: &str = "zhizi.ico";
const NUMERIC_VERSION_TOKEN: &str = "0,1,0,0";
const TEXT_VERSION_TOKEN: &str = "\"0.1.0\\0\"";
const ICON_LINE_TOKEN: &str = "1 ICON \"zhizi.ico\"";
const GENERATED_RC_NAME: &str = "zhizi.rc";

fn fail(message: String) -> ! {
    eprintln!();
    eprintln!("zhizi-tsf build script failed:");
    eprintln!("  {}", message);
    eprintln!();
    eprintln!("The build script refuses to continue rather than emitting wrong resource version metadata.");
    process::exit(1);
}

fn cargo_env(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => fail(format!(
            "environment variable `{}` is set but empty; run this build through cargo so Cargo-provided variables are populated.",
            name
        )),
        Err(env::VarError::NotPresent) => fail(format!(
            "environment variable `{}` is not present; run this build through cargo (e.g. `cargo build`), not by executing build.rs directly.",
            name
        )),
        Err(env::VarError::NotUnicode(raw)) => fail(format!(
            "environment variable `{}` is not valid Unicode ({:?}); it must be a usable path.",
            name, raw
        )),
    }
}

fn parse_release_version(raw: &str, version_path: &Path) -> [u16; 3] {
    let text = raw.trim();
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 3 {
        fail(format!(
            "{}: expected exactly three dot-separated numeric components (major.minor.patch, e.g. 0.3.45), found {:?}.",
            version_path.display(),
            text
        ));
    }
    let mut parsed = [0u16; 3];
    for (slot, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            fail(format!(
                "{}: component {} of version {:?} is {:?}; each component must consist solely of ASCII digits 0-9 (no signs, spaces, or other characters).",
                version_path.display(),
                slot + 1,
                text,
                part
            ));
        }
        match part.parse::<u16>() {
            Ok(value) => parsed[slot] = value,
            Err(err) => fail(format!(
                "{}: component {} of version {:?} is out of range for a u16 resource field (0-65535): {}.",
                version_path.display(),
                slot + 1,
                text,
                err
            )),
        }
    }
    parsed
}

fn apply_exact_replacement(
    text: String,
    token: &str,
    replacement: &str,
    label: &str,
    template_path: &Path,
) -> String {
    let occurrences = text.matches(token).count();
    if occurrences == 0 {
        fail(format!(
            "resource template {} does not contain the expected {} placeholder {:?}; align the template with the build script.",
            template_path.display(),
            label,
            token
        ));
    }
    text.replace(token, replacement)
}

fn rc_friendly_path(path: &Path) -> String {
    let text = path.to_str().unwrap_or_else(|| fail("icon path is not valid Unicode".into())).to_owned();
    if text.chars().any(|c| c == '"' || c.is_control()) { fail("icon path contains characters unsafe in an RC string".into()); }
    let without_verbatim = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!("\\\\{}", rest)
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    };
    without_verbatim.replace('\\', "/")
}

fn main() {
    let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(cargo_env("OUT_DIR"));

    let root_dir = manifest_dir
        .ancestors()
        .nth(3)
        .unwrap_or_else(|| {
            fail(format!(
                "cannot determine repository root: the crate manifest directory {} does not have three parent directories.",
                manifest_dir.display()
            ))
        });
    let version_path = root_dir.join("VERSION");
    let version_raw = fs::read_to_string(&version_path).unwrap_or_else(|err| {
        fail(format!(
            "cannot read root VERSION file {}: {}.",
            version_path.display(),
            err
        ))
    });
    let [major, minor, patch] = parse_release_version(&version_raw, &version_path);
    let release_version = format!("{}.{}.{}", major, minor, patch);

    let template_path = manifest_dir.join(RC_TEMPLATE_REL);
    let template = fs::read_to_string(&template_path).unwrap_or_else(|err| {
        fail(format!(
            "cannot read resource template {}: {}.",
            template_path.display(),
            err
        ))
    });

    let icon_source = template_path
        .parent()
        .unwrap_or(&manifest_dir)
        .join(ICON_FILE_NAME);
    let icon_canonical = fs::canonicalize(&icon_source).unwrap_or_else(|err| {
        fail(format!(
            "cannot locate icon file {}: {}.",
            icon_source.display(),
            err
        ))
    });
    let icon_rc_path = rc_friendly_path(&icon_canonical);

    println!("cargo:rustc-env=ZHIZI_RELEASE_VERSION={}", release_version);
    println!("cargo:rerun-if-changed={}", version_path.display());
    println!("cargo:rerun-if-changed={}", template_path.display());
    println!("cargo:rerun-if-changed={}", icon_canonical.display());
    println!("cargo:rerun-if-changed=build.rs");

    let numeric_replacement = format!("{},{},{},{}", major, minor, patch, 0);
    let text_replacement = format!("\"{}\\0\"", release_version);
    let icon_replacement = format!("1 ICON \"{}\"", icon_rc_path);

    let generated = apply_exact_replacement(
        template,
        NUMERIC_VERSION_TOKEN,
        &numeric_replacement,
        "numeric FILEVERSION/PRODUCTVERSION",
        &template_path,
    );
    let generated = apply_exact_replacement(
        generated,
        TEXT_VERSION_TOKEN,
        &text_replacement,
        "text FileVersion/ProductVersion",
        &template_path,
    );
    let generated = apply_exact_replacement(
        generated,
        ICON_LINE_TOKEN,
        &icon_replacement,
        "icon entry",
        &template_path,
    );

    let generated_path = out_dir.join(GENERATED_RC_NAME);
    fs::write(&generated_path, &generated).unwrap_or_else(|err| {
        fail(format!(
            "cannot write generated resource script {}: {}.",
            generated_path.display(),
            err
        ))
    });

    let generated_rc = generated_path.to_str().unwrap_or_else(|| {
        fail(format!(
            "generated resource script path {} is not valid Unicode.",
            generated_path.display()
        ))
    });
    embed_resource::compile(generated_rc, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|error| fail(format!("embedded resource compilation failed: {error:?}")));
}

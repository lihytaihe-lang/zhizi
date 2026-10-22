use std::sync::atomic::AtomicU64;

#[cfg(unix)]
#[path = "custom_config_unix.rs"]
mod store;
#[cfg(windows)]
#[path = "custom_config_windows.rs"]
mod store;

pub(crate) use store::CustomConfigStore;

const CONFIG_FILE_NAME: &str = "agent_pinyin.custom.yaml";
const MAXIMUM_CONFIG_BYTES: usize = 64 * 1024;
const TEMPORARY_ATTEMPTS: usize = 64;


static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

const BASE_SCHEMA: &str = include_str!("../../../resources/Rime/agent_pinyin.schema.yaml");

fn base_algebra() -> Vec<&'static str> {
    let mut in_algebra = false;
    let mut rules = Vec::new();
    for line in BASE_SCHEMA.lines() {
        if line == "  algebra:" {
            in_algebra = true;
            continue;
        }
        if !in_algebra {
            continue;
        }
        if line.starts_with("    - ") {
            rules.push(line);
        } else if !line.trim().is_empty() && !line.starts_with("    #") {
            break;
        }
    }
    assert!(
        !rules.is_empty(),
        "agent_pinyin base algebra must not be empty"
    );
    rules
}

pub(crate) fn fuzzy_config(mask: u32) -> String {
    let mut algebra = base_algebra();
    // Fuzzy spellings must remain real derivations. `abbrev` only accepts a
    // prefix of the canonical spelling and therefore cannot implement pairs
    // such as z -> zh or an -> ang for arbitrary full syllables. The canonical
    // side is normalized once with `xform`; the reverse spelling is then
    // generated with `derive`, avoiding a cyclic pair of derivations.
    let rules: &[(u32, &[&str])] = &[
        (1 << 0, &["    - xform/^zh/z/", "    - derive/^z/zh/"]),
        (1 << 1, &["    - xform/^ch/c/", "    - derive/^c/ch/"]),
        (1 << 2, &["    - xform/^sh/s/", "    - derive/^s/sh/"]),
        (1 << 3, &["    - xform/^n/l/", "    - derive/^l/n/"]),
        (1 << 4, &["    - xform/^f/h/", "    - derive/^h/f/"]),
        (1 << 5, &["    - xform/ang$/an/", "    - derive/an$/ang/"]),
        (1 << 6, &["    - xform/eng$/en/", "    - derive/en$/eng/"]),
        (1 << 7, &["    - xform/ing$/in/", "    - derive/in$/ing/"]),
    ];
    for (bit, additions) in rules {
        if mask & bit != 0 {
            algebra.extend_from_slice(additions);
        }
    }
    format!(
        "patch:\n  schema/version: \"0.3.0.fuzzy.{mask}\"\n  speller/algebra:\n{}\n",
        algebra.join("\n")
    )
}

fn legacy_fuzzy_config_v2(mask: u32) -> String {
    let mut algebra = vec![
        "    - abbrev/^([a-z]).+$/$1/",
        "    - abbrev/^([zcs]h).+$/$1/",
        "    - derive/^([nl])ue$/$1ve/",
        "    - derive/^([jqxy])u/$1v/",
        "    - derive/un$/uen/",
        "    - derive/ui$/uei/",
        "    - derive/iu$/iou/",
    ];
    let rules: &[(u32, &[&str])] = &[
        (1 << 0, &["    - abbrev/^zh/z/", "    - abbrev/^z/zh/"]),
        (1 << 1, &["    - abbrev/^ch/c/", "    - abbrev/^c/ch/"]),
        (1 << 2, &["    - abbrev/^sh/s/", "    - abbrev/^s/sh/"]),
        (1 << 3, &["    - abbrev/^n/l/", "    - abbrev/^l/n/"]),
        (1 << 4, &["    - abbrev/^f/h/", "    - abbrev/^h/f/"]),
        (1 << 5, &["    - abbrev/ang$/an/", "    - abbrev/an$/ang/"]),
        (1 << 6, &["    - abbrev/eng$/en/", "    - abbrev/en$/eng/"]),
        (1 << 7, &["    - abbrev/ing$/in/", "    - abbrev/in$/ing/"]),
    ];
    for (bit, additions) in rules {
        if mask & bit != 0 {
            algebra.extend_from_slice(additions);
        }
    }
    format!(
        "patch:\n  schema/version: \"0.3.0.fuzzy.{mask}\"\n  speller/algebra:\n{}\n",
        algebra.join("\n")
    )
}

fn legacy_fuzzy_config_v1(mask: u32) -> String {
    let mut algebra = vec![
        "    - abbrev/^([a-z]).+$/$1/",
        "    - abbrev/^([zcs]h).+$/$1/",
        "    - derive/^([nl])ue$/$1ve/",
        "    - derive/^([jqxy])u/$1v/",
        "    - derive/un$/uen/",
        "    - derive/ui$/uei/",
        "    - derive/iu$/iou/",
    ];
    let rules: &[(u32, &[&str])] = &[
        (1 << 0, &["    - xform/^zh/z/", "    - derive/^z/zh/"]),
        (1 << 1, &["    - xform/^ch/c/", "    - derive/^c/ch/"]),
        (1 << 2, &["    - xform/^sh/s/", "    - derive/^s/sh/"]),
        (1 << 3, &["    - xform/^n/l/", "    - derive/^l/n/"]),
        (1 << 4, &["    - xform/^f/h/", "    - derive/^h/f/"]),
        (1 << 5, &["    - xform/ang$/an/", "    - derive/an$/ang/"]),
        (1 << 6, &["    - xform/eng$/en/", "    - derive/en$/eng/"]),
        (1 << 7, &["    - xform/ing$/in/", "    - derive/in$/ing/"]),
    ];
    for (bit, additions) in rules {
        if mask & bit != 0 {
            algebra.extend_from_slice(additions);
        }
    }
    format!(
        "patch:\n  schema/version: \"0.3.0.fuzzy.{mask}\"\n  speller/algebra:\n{}\n",
        algebra.join("\n")
    )
}

pub(crate) fn is_managed_fuzzy_config(contents: &[u8]) -> bool {
    const PREFIX: &str = "patch:\n  schema/version: \"0.3.0.fuzzy.";
    let Ok(text) = std::str::from_utf8(contents) else {
        return false;
    };
    let Some(remainder) = text.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((mask, _)) = remainder.split_once('"') else {
        return false;
    };
    let Ok(mask) = mask.parse::<u32>() else {
        return false;
    };
    mask != 0
        && mask <= 0xff
        && (fuzzy_config(mask).as_bytes() == contents
            || legacy_fuzzy_config_v2(mask).as_bytes() == contents
            || legacy_fuzzy_config_v1(mask).as_bytes() == contents)
}

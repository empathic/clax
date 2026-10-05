//! The Chrome extension built into this binary (`web/dist-extension`, the
//! release build), and where each supported browser looks for native
//! messaging hosts (spec 2026-10-05 §5.4). `build.rs` rebuilds the binary
//! when the build changes.

use rust_embed::RustEmbed;
use std::path::PathBuf;

#[derive(RustEmbed)]
#[folder = "../../web/dist-extension/"]
#[exclude = ".gitkeep"]
#[exclude = "key/*"]
#[exclude = "**/.DS_Store"]
struct Dist;

/// Every file of the extension: (path, contents), sorted. A debug build
/// reads `CLAX_EXTENSION_DIST` instead when it is set (the tests' fixture).
/// Empty when this binary was built before `web/dist-extension` was.
pub fn files() -> Vec<(String, Vec<u8>)> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("CLAX_EXTENSION_DIST") {
        return walk(&PathBuf::from(dir));
    }
    let mut out: Vec<(String, Vec<u8>)> = Dist::iter()
        .map(|p| {
            let f = Dist::get(&p).expect("an embedded file lists itself");
            (p.to_string(), f.data.into_owned())
        })
        .collect();
    out.sort();
    out
}

#[cfg(debug_assertions)]
fn walk(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let (Ok(rel), Ok(bytes)) = (p.strip_prefix(root), std::fs::read(&p)) {
                out.push((rel.to_string_lossy().replace('\\', "/"), bytes));
            }
        }
    }
    out.sort();
    out
}

/// One browser's native messaging hosts directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDir {
    pub browser: String,
    pub dir: PathBuf,
}

/// Profile directories under `~/Library/Application Support` (macOS).
const MAC: &[(&str, &str)] = &[
    ("chrome", "Google/Chrome"),
    ("chrome-beta", "Google/Chrome Beta"),
    ("chrome-dev", "Google/Chrome Dev"),
    ("chrome-canary", "Google/Chrome Canary"),
    ("chromium", "Chromium"),
    ("brave", "BraveSoftware/Brave-Browser"),
    ("edge", "Microsoft Edge"),
];
/// Profile directories under `$XDG_CONFIG_HOME`, else `~/.config` (Linux).
const LINUX: &[(&str, &str)] = &[
    ("chrome", "google-chrome"),
    ("chrome-beta", "google-chrome-beta"),
    ("chrome-dev", "google-chrome-unstable"),
    ("chromium", "chromium"),
    ("brave", "BraveSoftware/Brave-Browser"),
    ("edge", "microsoft-edge"),
];

/// Every browser's hosts directory, whether or not the browser is
/// installed (`install` writes only where the profile directory, the hosts
/// directory's parent, exists). `CLAX_NATIVE_HOST_DIRS`
/// (`<browser>=<dir>:<browser>=<dir>…`) replaces the table; each listed
/// directory then stands for its browser's profile.
pub fn host_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<HostDir> {
    if let Some(list) = env("CLAX_NATIVE_HOST_DIRS") {
        return list
            .split(':')
            .filter_map(|e| e.split_once('='))
            .filter(|(b, d)| !b.is_empty() && !d.is_empty())
            .map(|(b, d)| HostDir {
                browser: b.to_string(),
                dir: PathBuf::from(d),
            })
            .collect();
    }
    let home = PathBuf::from(env("HOME").unwrap_or_default());
    let (base, table) = if cfg!(target_os = "macos") {
        (home.join("Library/Application Support"), MAC)
    } else {
        let base = env("XDG_CONFIG_HOME")
            .filter(|x| x.starts_with('/'))
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        (base, LINUX)
    };
    table
        .iter()
        .map(|(b, d)| HostDir {
            browser: b.to_string(),
            dir: base.join(d).join("NativeMessagingHosts"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_directories_follow_each_os() {
        let env = |k: &str| match k {
            "HOME" => Some("/h".to_string()),
            _ => None,
        };
        let dirs = host_dirs(&env);
        let paths: Vec<String> = dirs.iter().map(|d| d.dir.display().to_string()).collect();
        if cfg!(target_os = "macos") {
            assert!(paths.contains(
                &"/h/Library/Application Support/Google/Chrome/NativeMessagingHosts".to_string()
            ));
            assert!(paths.contains(
                &"/h/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts"
                    .to_string()
            ));
            assert!(paths.contains(
                &"/h/Library/Application Support/Microsoft Edge/NativeMessagingHosts".to_string()
            ));
        } else {
            assert!(paths.contains(&"/h/.config/google-chrome/NativeMessagingHosts".to_string()));
            assert!(paths.contains(&"/h/.config/chromium/NativeMessagingHosts".to_string()));
        }
        let over =
            |k: &str| (k == "CLAX_NATIVE_HOST_DIRS").then(|| "chrome=/a:brave=/b".to_string());
        assert_eq!(
            host_dirs(&over),
            vec![
                HostDir {
                    browser: "chrome".into(),
                    dir: "/a".into()
                },
                HostDir {
                    browser: "brave".into(),
                    dir: "/b".into()
                },
            ]
        );
    }

    #[test]
    fn the_embedded_build_carries_no_placeholder_or_key() {
        assert!(
            Dist::iter().all(|p| p != ".gitkeep" && !p.starts_with("key/")),
            "{:?}",
            Dist::iter().collect::<Vec<_>>()
        );
    }

    /// `build.rs` names every directory a file is embedded from, so a
    /// rebuilt extension rebuilds the binary.
    #[test]
    fn the_build_script_watches_every_embedded_directory() {
        let watched: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/plugin-dirs.txt"))
            .lines()
            .collect();
        assert!(watched.contains(&"web/dist-extension"), "{watched:?}");
        for p in Dist::iter() {
            for d in std::path::Path::new(p.as_ref()).ancestors().skip(1) {
                let d = std::path::Path::new("web/dist-extension").join(d);
                let d = d.to_str().unwrap().trim_end_matches('/');
                assert!(watched.contains(&d), "{d} (for {p}) is not watched");
            }
        }
    }
}

//! Where a Chromium-family browser is looked for, in order, and the messages when none is found.

use std::path::{Path, PathBuf};

use wg_procspec::executable_suffixes;
use wg_types::{ErrorCode, WgError};

/// The environment variable that names the browser executable.
pub const BROWSER_ENV: &str = "WORKGRAPH_BROWSER";

/// The executable names tried on `PATH`, in order.
pub const SEARCH_NAMES: [&str; 6] = [
    "chrome",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "msedge",
];

/// The operating system whose search rules apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Other,
}

impl Platform {
    /// The platform this binary was built for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    #[must_use]
    pub const fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }
}

/// The browser asked for: the explicit argument, else the `WORKGRAPH_BROWSER` value. Empty
/// values count as unset, as in Python's `or`.
#[must_use]
pub fn requested_browser(explicit: Option<&str>, env_value: Option<&str>) -> Option<String> {
    [explicit, env_value]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The candidate paths of a browser named explicitly: the name as a path first, then its
/// `PATH` matches.
#[must_use]
pub fn explicit_candidates(
    chosen: &str,
    path_var: &str,
    pathext: Option<&str>,
    platform: Platform,
) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::from(chosen)];
    found.extend(path_candidates(chosen, path_var, pathext, platform));
    found
}

/// The `PATH` matches of every name in [`SEARCH_NAMES`], in search order.
#[must_use]
pub fn search_candidates(
    path_var: &str,
    pathext: Option<&str>,
    platform: Platform,
) -> Vec<PathBuf> {
    SEARCH_NAMES
        .iter()
        .flat_map(|name| path_candidates(name, path_var, pathext, platform))
        .collect()
}

/// Where `name` could be on `PATH`, directory by directory, each with the executable suffixes
/// the platform tries (as `shutil.which` does).
#[must_use]
pub fn path_candidates(
    name: &str,
    path_var: &str,
    pathext: Option<&str>,
    platform: Platform,
) -> Vec<PathBuf> {
    let windows = platform.is_windows();
    let separator = if windows { ';' } else { ':' };
    let extension = Path::new(name).extension().and_then(|ext| ext.to_str());
    let suffixes = executable_suffixes(extension, pathext, windows);
    path_var
        .split(separator)
        .filter(|dir| !dir.is_empty())
        .flat_map(|dir| {
            suffixes
                .iter()
                .map(move |suffix| PathBuf::from(dir).join(format!("{name}{suffix}")))
        })
        .collect()
}

/// The standard install locations of Chrome, Edge and Chromium for `platform`, in search order.
/// Windows reads its roots through `lookup` and skips the ones that are unset or empty.
pub fn install_locations(
    platform: Platform,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    match platform {
        Platform::Windows => {
            let relative = [
                "Google/Chrome/Application/chrome.exe",
                "Microsoft/Edge/Application/msedge.exe",
                "Chromium/Application/chrome.exe",
            ];
            ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
                .into_iter()
                .filter_map(|key| lookup(key).filter(|root| !root.is_empty()))
                .flat_map(|root| {
                    relative
                        .into_iter()
                        .map(move |path| PathBuf::from(&root).join(path))
                })
                .collect()
        }
        Platform::MacOs => vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ],
        Platform::Other => Vec::new(),
    }
}

/// The browser to drive: the requested one if any, else the first installed one.
///
/// `env` reads environment variables and `is_file` says whether a path is an existing file, so
/// the search order is a pure function of their answers.
///
/// # Errors
/// [`browser_not_found`] for a requested browser that is not a file, [`no_browser_installed`]
/// when nothing is requested and nothing is found.
pub fn locate_browser(
    explicit: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    is_file: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, WgError> {
    let platform = Platform::current();
    let path_var = env("PATH").unwrap_or_default();
    let pathext = env("PATHEXT");
    let env_browser = env(BROWSER_ENV);
    let first_file = |candidates: Vec<PathBuf>| candidates.into_iter().find(|path| is_file(path));
    match requested_browser(explicit, env_browser.as_deref()) {
        Some(chosen) => first_file(explicit_candidates(
            &chosen,
            &path_var,
            pathext.as_deref(),
            platform,
        ))
        .ok_or_else(|| browser_not_found(&chosen)),
        None => {
            let mut candidates = search_candidates(&path_var, pathext.as_deref(), platform);
            candidates.extend(install_locations(platform, env));
            first_file(candidates).ok_or_else(no_browser_installed)
        }
    }
}

/// The error for a browser named explicitly that does not exist.
#[must_use]
pub fn browser_not_found(chosen: &str) -> WgError {
    WgError::new(
        ErrorCode::ProviderUnavailable,
        format!("browser not found: {chosen}"),
    )
}

/// The error when no browser is named and none is installed where it is looked for.
#[must_use]
pub fn no_browser_installed() -> WgError {
    WgError::new(
        ErrorCode::ProviderUnavailable,
        format!(
            "no Chrome, Edge or Chromium found; install one or set {BROWSER_ENV} to its executable"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn explicit_beats_the_environment_and_blank_values_are_unset() {
        assert_eq!(
            requested_browser(Some("a"), Some("b")).as_deref(),
            Some("a")
        );
        assert_eq!(requested_browser(Some(""), Some("b")).as_deref(), Some("b"));
        assert_eq!(requested_browser(None, Some("")), None);
        assert_eq!(requested_browser(None, None), None);
    }

    #[test]
    fn path_candidates_walk_directories_in_order() {
        let found = path_candidates("chrome", "/a:/b", None, Platform::Other);
        assert_eq!(
            found,
            vec![PathBuf::from("/a/chrome"), PathBuf::from("/b/chrome")]
        );
    }

    #[test]
    fn windows_path_tries_pathext_suffixes() {
        let found = path_candidates("msedge", "C:\\bin", Some(".EXE;.CMD"), Platform::Windows);
        assert_eq!(
            found,
            vec![
                PathBuf::from("C:\\bin").join("msedge.EXE"),
                PathBuf::from("C:\\bin").join("msedge.CMD"),
            ]
        );
    }

    #[test]
    fn search_order_is_names_then_install_locations() {
        let names = search_candidates("/x", None, Platform::Other);
        assert_eq!(names.len(), SEARCH_NAMES.len());
        assert_eq!(names[0], PathBuf::from("/x/chrome"));
        assert_eq!(names[5], PathBuf::from("/x/msedge"));
        assert!(install_locations(Platform::Other, none).is_empty());
    }

    #[test]
    fn windows_install_locations_are_root_major_and_skip_empty_roots() {
        let lookup = |key: &str| match key {
            "PROGRAMFILES" => Some("C:\\PF".to_owned()),
            "LOCALAPPDATA" => Some(String::new()),
            _ => None,
        };
        let found = install_locations(Platform::Windows, lookup);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found[0],
            PathBuf::from("C:\\PF").join("Google/Chrome/Application/chrome.exe")
        );
        assert_eq!(
            found[2],
            PathBuf::from("C:\\PF").join("Chromium/Application/chrome.exe")
        );
    }

    #[test]
    fn locate_prefers_the_requested_browser_and_reports_misses() {
        let fake = PathBuf::from("/opt/my-chrome");
        let env = |key: &str| match key {
            "PATH" => Some("/usr/bin".to_owned()),
            BROWSER_ENV => Some("/opt/my-chrome".to_owned()),
            _ => None,
        };
        let is_fake = |path: &Path| path == fake.as_path();
        assert_eq!(locate_browser(None, &env, &is_fake), Ok(fake.clone()));
        let missing = locate_browser(Some("nope"), &env, &is_fake).expect_err("nope is not a file");
        assert_eq!(missing.message, "browser not found: nope");
        let none = locate_browser(None, &none_env, &|_| false).expect_err("nothing installed");
        assert!(
            none.message
                .starts_with("no Chrome, Edge or Chromium found")
        );
    }

    fn none_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn messages_name_the_environment_variable() {
        assert_eq!(browser_not_found("x").message, "browser not found: x");
        assert!(no_browser_installed().message.contains(BROWSER_ENV));
    }
}

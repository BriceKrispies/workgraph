//! The built web UI: embedded at build time from Vite's output directory
//! (`web/dist`), read from that directory at run time in
//! debug builds, or read from a directory given at run time.

use std::path::PathBuf;

use rust_embed::RustEmbed;

/// Vite's output directory. A missing directory embeds nothing: the server then shows the
/// "web UI has not been built" page.
#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct Embedded;

/// Where the UI files come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ui {
    /// The files embedded in the binary (in debug builds, the build directory read live).
    Embedded,
    /// The files of a directory.
    Dir(PathBuf),
}

impl Ui {
    /// The bytes of the file at `relative` (`/`-separated, inside the UI, from
    /// `crate::spec::spa::static_path`), or `None` when there is no such file.
    pub fn file(&self, relative: &str) -> Option<Vec<u8>> {
        match self {
            Ui::Embedded => Embedded::get(relative).map(|file| file.data.into_owned()),
            Ui::Dir(root) => {
                let path = relative
                    .split('/')
                    .fold(root.clone(), |path, segment| path.join(segment));
                std::fs::metadata(&path)
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .and_then(|_| std::fs::read(&path).ok())
            }
        }
    }
}

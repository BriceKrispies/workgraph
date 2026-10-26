//! The change surface of a range (Pure tier): the decisions of `compute_change_surface`'s loop over
//! the inputs the shell read. The shell runs `git diff --numstat`, and for each text file its hunks
//! and its sources at both revisions; this module analyses each file's structure and builds the
//! surface record.

use std::fmt::Display;

use wg_codeanalysis::{FileStructure, Language, Mode, Symbol, analyze_file, parse_hunks};
use wg_gitcore::{FileChange, Provenance, SurfaceRecord};

use super::reads::to_u64;

/// What the shell read of one changed text file.
pub struct FileInput {
    /// The unified diff of the file between the two revisions, without context lines.
    pub hunks: String,
    /// The file's source at the base revision (`None` when the file is absent there).
    pub old_source: Option<Vec<u8>>,
    /// The file's source at the final revision (`None` when the file is absent there).
    pub new_source: Option<Vec<u8>>,
}

/// The arguments of the hunk read of one file: its diff without context lines, from the base to
/// the final revision, limited to its old and new paths.
#[must_use]
pub fn hunk_args<'a>(base: &'a str, final_rev: &'a str, file: &'a FileChange) -> Vec<&'a str> {
    let mut args: Vec<&str> = vec!["diff", "-U0", "--no-color", "-M", base, final_rev, "--"];
    if let Some(old) = file.old_path.as_deref() {
        args.push(old);
    }
    args.push(file.path.as_str());
    args
}

/// The path whose source is read at the base revision: the old path of a renamed file.
#[must_use]
pub fn base_path(file: &FileChange) -> &str {
    file.old_path.as_deref().unwrap_or(&file.path)
}

/// The surface record of a range. `inputs[i]` is what the shell read of `files[i]`; it is `None`
/// for a binary file, which is not analysed.
pub fn surface_from_inputs<E: Display>(
    base: &str,
    final_rev: &str,
    provenance: Provenance,
    files: &[FileChange],
    inputs: &[Option<FileInput>],
    extract: impl Fn(&[u8], Language) -> Result<Option<Vec<Symbol>>, E> + Copy,
) -> SurfaceRecord {
    let structures: Vec<Option<(u64, u64)>> = files
        .iter()
        .zip(inputs)
        .map(|(file, input)| {
            let input = input.as_ref()?;
            let (deleted, added) = parse_hunks(&input.hunks);
            let structure: FileStructure = analyze_file(
                &file.path,
                wg_gitcore::language_for(&file.path),
                input.old_source.as_deref(),
                input.new_source.as_deref(),
                &deleted,
                &added,
                extract,
            );
            match structure.mode {
                Mode::Structural => {
                    Some((to_u64(structure.functions()), to_u64(structure.types())))
                }
                Mode::Fallback => None,
            }
        })
        .collect();
    wg_gitcore::surface_record(base, final_rev, provenance, files, &structures)
}

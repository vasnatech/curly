//! A "project" — a curly [`Storage`] root plus a display label for the UI.
//! Not a new concept on disk: it's exactly what the CLI already calls a
//! project-local `.curly` directory (`curly init`), or the OS-wide default
//! location when no such directory has been opened. Ported near-verbatim
//! from `crates/curly-gui/src/project.rs` (the egui GUI's equivalent) — see
//! that file's own doc comment for why a `Vec<Project>`-shaped future isn't
//! attempted here yet either; this scoping pass only carries over the
//! single-project labeling logic, not multi-project support (M7).

use std::path::Path;

use curly_core::storage::Storage;
use serde::Serialize;

/// What a command returns to the frontend after opening/resolving a
/// project — `root` is stringified (`Path::display`) rather than left as a
/// `PathBuf` to avoid relying on serde's platform-dependent path encoding,
/// matching how the egui GUI already stringifies it for display.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    pub label: String,
    pub root: String,
}

pub fn info_for(storage: &Storage) -> ProjectInfo {
    ProjectInfo {
        label: label_for(storage),
        root: storage.root().display().to_string(),
    }
}

/// "Default" for the OS-wide default location, otherwise the parent
/// directory's name (the project root a `.curly` sits inside) — falling
/// back to the raw path if a directory name can't be extracted (e.g. `.curly`
/// itself is the filesystem root, which shouldn't happen in practice).
fn label_for(storage: &Storage) -> String {
    if storage.is_default_location() {
        return "Default".to_string();
    }
    project_root_name(storage.root()).unwrap_or_else(|| storage.root().display().to_string())
}

/// Given a `.curly` directory's path, name the project after its parent
/// (`/home/user/my-api/.curly` -> `"my-api"`).
fn project_root_name(curly_dir: &Path) -> Option<String> {
    let parent = if curly_dir.file_name().and_then(|n| n.to_str()) == Some(".curly") {
        curly_dir.parent()?
    } else {
        curly_dir
    };
    parent.file_name().map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn default_location_labeled_default() {
        let storage = Storage::default_location().unwrap();
        assert_eq!(info_for(&storage).label, "Default");
    }

    #[test]
    fn project_local_storage_labeled_by_parent_dir_name() {
        let storage = Storage::new(PathBuf::from("/home/user/my-api/.curly"));
        assert_eq!(info_for(&storage).label, "my-api");
    }

    #[test]
    fn non_dot_curly_root_labeled_by_its_own_dir_name() {
        // an explicit --data-dir-style override that isn't named .curly
        let storage = Storage::new(PathBuf::from("/home/user/scratch-storage"));
        assert_eq!(info_for(&storage).label, "scratch-storage");
    }
}

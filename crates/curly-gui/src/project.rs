//! A "project" — a curly [`Storage`] root plus a display label for the UI.
//! Not a new concept on disk: it's exactly what the CLI already calls a
//! project-local `.curly` directory (`curly init`), or the OS-wide default
//! location when no such directory has been opened.
//!
//! `CurlyApp` holds a `Vec<Project>` + an active index rather than a single
//! `Option<Project>`, even though only one can be open at a time today
//! (opening a project replaces the vec's one entry) — see DESIGN.md's M4
//! entry: the user asked for this specifically so that real multi-project
//! support (tabs, switching, closing one without losing the others) is an
//! additive change to the UI/interaction layer later, not a data-model
//! migration.

use std::path::{Path, PathBuf};

use curly_core::storage::Storage;

pub struct Project {
    pub label: String,
    pub root: PathBuf,
    pub storage: Storage,
}

impl Project {
    pub fn from_storage(storage: Storage) -> Self {
        let label = label_for(&storage);
        let root = storage.root().to_path_buf();
        Self {
            label,
            root,
            storage,
        }
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
    parent
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_location_labeled_default() {
        let storage = Storage::default_location().unwrap();
        let project = Project::from_storage(storage);
        assert_eq!(project.label, "Default");
    }

    #[test]
    fn project_local_storage_labeled_by_parent_dir_name() {
        let storage = Storage::new(PathBuf::from("/home/user/my-api/.curly"));
        let project = Project::from_storage(storage);
        assert_eq!(project.label, "my-api");
    }

    #[test]
    fn non_dot_curly_root_labeled_by_its_own_dir_name() {
        // an explicit --data-dir-style override that isn't named .curly
        let storage = Storage::new(PathBuf::from("/home/user/scratch-storage"));
        let project = Project::from_storage(storage);
        assert_eq!(project.label, "scratch-storage");
    }
}

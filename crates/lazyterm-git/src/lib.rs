use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoContext {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub has_changes: bool,
}

impl RepoContext {
    pub fn unknown(cwd: impl AsRef<Path>) -> Self {
        Self {
            root: cwd.as_ref().to_path_buf(),
            branch: None,
            has_changes: false,
        }
    }
}

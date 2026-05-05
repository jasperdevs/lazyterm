use std::path::PathBuf;

use lazyterm_git::RepoContext;

#[test]
fn unknown_repo_context_preserves_requested_root() {
    let context = RepoContext::unknown(PathBuf::from("repo"));

    assert_eq!(context.root, PathBuf::from("repo"));
    assert_eq!(context.branch, None);
    assert!(!context.has_changes);
}

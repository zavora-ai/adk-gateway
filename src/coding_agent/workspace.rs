//! Workspace path restriction and security validation.
//!
//! Ensures coding agents can only operate within their configured workspace
//! directories. Handles path canonicalization, symlink resolution, and
//! directory traversal prevention.

use std::path::{Path, PathBuf};

use crate::coding_agent::error::CodingAgentError;

/// Validates that file paths are within allowed workspace directories.
///
/// The validator stores canonicalized workspace paths and checks that any
/// given path resolves to a descendant of at least one allowed workspace.
/// This prevents directory traversal attacks (`..`), symlink escapes, and
/// access to files outside the configured workspaces.
#[derive(Debug, Clone)]
pub struct WorkspaceValidator {
    /// Canonicalized allowed workspace directories.
    allowed_workspaces: Vec<PathBuf>,
}

impl WorkspaceValidator {
    /// Creates a new `WorkspaceValidator` from a list of allowed workspace paths.
    ///
    /// Each path is canonicalized at construction time. Paths that cannot be
    /// canonicalized (e.g., non-existent directories) are skipped with a warning.
    pub fn new(workspaces: &[PathBuf]) -> Self {
        let allowed_workspaces = workspaces
            .iter()
            .filter_map(|p| match p.canonicalize() {
                Ok(canonical) => Some(canonical),
                Err(e) => {
                    tracing::warn!(
                        path = %p.display(),
                        error = %e,
                        "Failed to canonicalize workspace path, skipping"
                    );
                    None
                }
            })
            .collect();

        Self { allowed_workspaces }
    }

    /// Creates a `WorkspaceValidator` from pre-canonicalized paths.
    ///
    /// Use this when you already have canonical paths (e.g., in tests)
    /// and don't need filesystem resolution.
    pub fn from_canonical(workspaces: Vec<PathBuf>) -> Self {
        Self {
            allowed_workspaces: workspaces,
        }
    }

    /// Returns the list of allowed workspace directories.
    pub fn allowed_workspaces(&self) -> &[PathBuf] {
        &self.allowed_workspaces
    }

    /// Validates that a single path is within an allowed workspace.
    ///
    /// The path is canonicalized (resolving symlinks and `..` components)
    /// and then checked to ensure it starts with at least one allowed
    /// workspace directory.
    ///
    /// # Errors
    ///
    /// Returns `CodingAgentError::WorkspaceViolation` if:
    /// - The path cannot be canonicalized (e.g., does not exist)
    /// - The canonicalized path is not a descendant of any allowed workspace
    pub fn validate_path(&self, path: &Path) -> Result<(), CodingAgentError> {
        // Canonicalize the path to resolve symlinks and `..` traversal
        let canonical = path.canonicalize().map_err(|_| {
            CodingAgentError::WorkspaceViolation {
                path: path.display().to_string(),
            }
        })?;

        // Check if the canonical path is a descendant of any allowed workspace
        if self.is_within_workspace(&canonical) {
            Ok(())
        } else {
            Err(CodingAgentError::WorkspaceViolation {
                path: path.display().to_string(),
            })
        }
    }

    /// Validates that all paths in a batch are within allowed workspaces.
    ///
    /// Returns an error on the first path that fails validation.
    ///
    /// # Errors
    ///
    /// Returns `CodingAgentError::WorkspaceViolation` with the first
    /// offending path if any path is outside the allowed workspaces.
    pub fn validate_paths(&self, paths: &[PathBuf]) -> Result<(), CodingAgentError> {
        for path in paths {
            self.validate_path(path)?;
        }
        Ok(())
    }

    /// Checks if a canonicalized path is a descendant of any allowed workspace.
    fn is_within_workspace(&self, canonical_path: &Path) -> bool {
        self.allowed_workspaces
            .iter()
            .any(|workspace| canonical_path.starts_with(workspace))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a temporary workspace with some files.
    fn setup_workspace() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();

        // Create some subdirectories and files
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::create_dir_all(workspace.join("src/nested")).unwrap();
        fs::write(workspace.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(workspace.join("src/nested/lib.rs"), "// lib").unwrap();

        (dir, workspace)
    }

    #[test]
    fn test_validate_path_within_workspace() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // File directly in workspace
        assert!(validator.validate_path(&workspace.join("src/main.rs")).is_ok());

        // Nested file
        assert!(validator
            .validate_path(&workspace.join("src/nested/lib.rs"))
            .is_ok());

        // Directory itself
        assert!(validator.validate_path(&workspace).is_ok());

        // Subdirectory
        assert!(validator.validate_path(&workspace.join("src")).is_ok());
    }

    #[test]
    fn test_validate_path_outside_workspace() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // Path outside workspace (use a known existing path)
        let outside_path = PathBuf::from("/tmp");
        let result = validator.validate_path(&outside_path);
        assert!(result.is_err());

        match result.unwrap_err() {
            CodingAgentError::WorkspaceViolation { path } => {
                assert_eq!(path, "/tmp");
            }
            other => panic!("Expected WorkspaceViolation, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_path_dot_dot_traversal() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // Attempt to traverse out of workspace using `..`
        // workspace/src/../../etc/passwd would resolve outside
        let traversal_path = workspace.join("src/../../../../../../tmp");

        // This should either fail canonicalization or be outside workspace
        let result = validator.validate_path(&traversal_path);
        // /tmp exists, so canonicalization succeeds but it's outside workspace
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_nonexistent_path() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // Non-existent path cannot be canonicalized
        let nonexistent = workspace.join("does/not/exist.rs");
        let result = validator.validate_path(&nonexistent);
        assert!(result.is_err());

        match result.unwrap_err() {
            CodingAgentError::WorkspaceViolation { path } => {
                assert!(path.contains("does/not/exist.rs"));
            }
            other => panic!("Expected WorkspaceViolation, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_path_symlink_within_workspace() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // Create a symlink within the workspace pointing to another file in workspace
        let target = workspace.join("src/main.rs");
        let link = workspace.join("src/link_to_main.rs");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &link).unwrap();
            // Symlink within workspace should be allowed
            assert!(validator.validate_path(&link).is_ok());
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_validate_path_symlink_escaping_workspace() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // Create a symlink inside workspace that points outside
        let link = workspace.join("src/escape_link");
        std::os::unix::fs::symlink("/tmp", &link).unwrap();

        // The symlink itself resolves to /tmp which is outside workspace
        let result = validator.validate_path(&link);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_paths_all_valid() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        let paths = vec![
            workspace.join("src/main.rs"),
            workspace.join("src/nested/lib.rs"),
        ];

        assert!(validator.validate_paths(&paths).is_ok());
    }

    #[test]
    fn test_validate_paths_one_invalid() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        let paths = vec![
            workspace.join("src/main.rs"),
            PathBuf::from("/tmp"), // outside workspace
        ];

        let result = validator.validate_paths(&paths);
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_workspaces() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let ws1 = dir1.path().to_path_buf();
        let ws2 = dir2.path().to_path_buf();

        // Create files in both workspaces
        fs::write(ws1.join("file1.rs"), "// ws1").unwrap();
        fs::write(ws2.join("file2.rs"), "// ws2").unwrap();

        let validator = WorkspaceValidator::new(&[ws1.clone(), ws2.clone()]);

        // Both should be valid
        assert!(validator.validate_path(&ws1.join("file1.rs")).is_ok());
        assert!(validator.validate_path(&ws2.join("file2.rs")).is_ok());

        // Outside both should fail
        let result = validator.validate_path(&PathBuf::from("/tmp"));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_workspaces_rejects_all() {
        let validator = WorkspaceValidator::from_canonical(vec![]);

        // With no allowed workspaces, everything should be rejected
        let result = validator.validate_path(&PathBuf::from("/tmp"));
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_root_itself_is_valid() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // The workspace root itself should be valid
        assert!(validator.validate_path(&workspace).is_ok());
    }

    #[test]
    fn test_from_canonical_constructor() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();

        fs::write(workspace.join("test.rs"), "// test").unwrap();

        let validator = WorkspaceValidator::from_canonical(vec![workspace.clone()]);

        assert!(validator.validate_path(&workspace.join("test.rs")).is_ok());
        assert_eq!(validator.allowed_workspaces().len(), 1);
    }

    #[test]
    fn test_nonexistent_workspace_is_skipped() {
        let dir = TempDir::new().unwrap();
        let valid_workspace = dir.path().to_path_buf();
        let invalid_workspace = PathBuf::from("/nonexistent/workspace/path/xyz123");

        let validator = WorkspaceValidator::new(&[valid_workspace.clone(), invalid_workspace]);

        // Only the valid workspace should be stored
        assert_eq!(validator.allowed_workspaces().len(), 1);
        assert!(validator.validate_path(&valid_workspace).is_ok());
    }

    #[test]
    fn test_relative_path_with_dot_dot() {
        let (_dir, workspace) = setup_workspace();
        let validator = WorkspaceValidator::new(&[workspace.clone()]);

        // A relative path that stays within workspace after resolution
        let path_with_dots = workspace.join("src/../src/main.rs");
        assert!(validator.validate_path(&path_with_dots).is_ok());
    }
}

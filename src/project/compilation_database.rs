use json_compilation_db::Entry;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Type alias for bidirectional path mappings
/// (original_path -> canonical_path, canonical_path -> original_path)
pub type PathMappings = (HashMap<PathBuf, PathBuf>, HashMap<PathBuf, PathBuf>);

#[derive(Error, Debug)]
pub enum CompilationDatabaseError {
    #[error("Compilation database file not found: {path}")]
    FileNotFound { path: String },
    #[error("Failed to read compilation database file: {error}")]
    ReadError { error: String },
    #[error("Failed to parse compilation database JSON: {error}")]
    ParseError { error: String },
    #[error("Compilation database is empty")]
    EmptyDatabase,
}

/// Wrapper around compilation database providing structured access to compilation entries
///
/// This struct contains both the path to the compilation database file and the parsed entries.
/// When serialized, only the path is included in the output to avoid serializing large database content.
#[derive(Debug, Clone, Deserialize)]
pub struct CompilationDatabase {
    /// Path to the compilation database file (compile_commands.json)
    pub path: PathBuf,
    /// Parsed compilation database entries (loaded at initialization)
    #[serde(skip)]
    pub entries: Vec<Entry>,
}

impl CompilationDatabase {
    /// Create a new compilation database by loading and parsing the file at the given path
    ///
    /// This immediately loads and parses the compilation database, returning an error if
    /// the file doesn't exist, can't be read, or contains invalid JSON.
    pub fn new(path: PathBuf) -> Result<Self, CompilationDatabaseError> {
        // Check if file exists
        if !path.exists() {
            return Err(CompilationDatabaseError::FileNotFound {
                path: path.to_string_lossy().to_string(),
            });
        }

        // Open and read the file
        let file = std::fs::File::open(&path).map_err(|e| CompilationDatabaseError::ReadError {
            error: e.to_string(),
        })?;

        // Parse the JSON compilation database
        let reader = std::io::BufReader::new(file);
        let entries: Vec<Entry> =
            serde_json::from_reader(reader).map_err(|e| CompilationDatabaseError::ParseError {
                error: e.to_string(),
            })?;

        // Check if database is empty
        if entries.is_empty() {
            return Err(CompilationDatabaseError::EmptyDatabase);
        }

        Ok(Self { path, entries })
    }

    /// Create a compilation database from entries for testing
    ///
    /// This bypasses filesystem operations and creates a CompilationDatabase
    /// directly from provided entries, useful for unit tests.
    #[cfg(test)]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self {
            path: PathBuf::from("/test/compile_commands.json"),
            entries,
        }
    }

    /// Create a compilation database from entries with a custom path for testing
    #[cfg(test)]
    pub fn from_entries_with_path(path: PathBuf, entries: Vec<Entry>) -> Self {
        Self { path, entries }
    }

    /// Get all compilation database entries
    #[allow(dead_code)]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Get the path to the compilation database file
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Get all unique source files with canonicalized paths
    ///
    /// This method resolves relative paths against each entry's `directory` field
    /// per the compile_commands.json spec, then canonicalizes them to absolute paths.
    pub fn canonical_source_files(&self) -> Result<Vec<PathBuf>, CompilationDatabaseError> {
        let mut canonical_files = Vec::new();
        let mut seen_files = std::collections::HashSet::new();

        for entry in &self.entries {
            let canonical_path = self.canonicalize_entry_path(&entry.file, &entry.directory)?;
            if seen_files.insert(canonical_path.clone()) {
                canonical_files.push(canonical_path);
            }
        }

        canonical_files.sort();
        Ok(canonical_files)
    }

    /// Get bidirectional mappings between original and canonical paths
    ///
    /// Returns (original -> canonical, canonical -> original) mappings.
    /// This enables efficient lookup in both directions without repeated canonicalization.
    pub fn path_mappings(&self) -> Result<PathMappings, CompilationDatabaseError> {
        let mut original_to_canonical = HashMap::new();
        let mut canonical_to_original = HashMap::new();

        for entry in &self.entries {
            let original_path = entry.file.clone();
            let canonical_path = self.canonicalize_entry_path(&entry.file, &entry.directory)?;

            original_to_canonical.insert(original_path.clone(), canonical_path.clone());
            canonical_to_original.insert(canonical_path, original_path);
        }

        Ok((original_to_canonical, canonical_to_original))
    }

    /// Canonicalize a single entry path using the entry's directory field
    ///
    /// Per the compile_commands.json spec, relative `file` paths must be resolved
    /// against the entry's `directory` field (the compilation working directory),
    /// NOT against the location of compile_commands.json itself.
    /// Falls back to compile_commands.json parent directory when entry directory is empty.
    fn canonicalize_entry_path(
        &self,
        entry_path: &Path,
        entry_directory: &Path,
    ) -> Result<PathBuf, CompilationDatabaseError> {
        // Per spec: resolve relative file paths against the entry's `directory` field.
        // Fall back to compile_commands.json parent when entry directory is empty.
        let base_dir = if entry_directory.as_os_str().is_empty() {
            self.path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        } else {
            entry_directory.to_path_buf()
        };

        let resolved_path = if entry_path.is_relative() {
            base_dir.join(entry_path)
        } else {
            entry_path.to_path_buf()
        };

        // Attempt canonicalization, fall back to logical normalization if it fails
        // (canonicalize requires the path to exist on disk)
        match resolved_path.canonicalize() {
            Ok(canonical) => Ok(canonical),
            Err(_) => Ok(normalize_path(&resolved_path)),
        }
    }
}

/// Logically normalize a path by resolving `.` and `..` components without filesystem access.
/// Unlike `canonicalize()`, this works on non-existent paths.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            std::path::Component::CurDir => {}
            _ => {
                components.push(component);
            }
        }
    }
    components.iter().collect()
}

/// Custom serialization that only outputs the path field
impl Serialize for CompilationDatabase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.path.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use json_compilation_db::Entry;
    use std::path::PathBuf;

    fn make_entry(directory: &str, file: &str) -> Entry {
        Entry {
            directory: PathBuf::from(directory),
            file: PathBuf::from(file),
            arguments: vec!["clang++".to_string(), file.to_string()],
            output: None,
        }
    }

    /// Normalize path to forward slashes for cross-platform assertion comparison
    fn norm(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn test_relative_path_resolved_against_entry_directory() {
        // Bug scenario: CDB is in /output/build/, but entry.directory points to /src/project/
        // Relative file path should resolve against /src/project/, NOT /output/build/
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/output/build/compile_commands.json"),
            vec![make_entry("/src/project", "../../lib/engine/main.cpp")],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 1);

        let resolved_str = norm(&files[0]);
        assert!(
            !resolved_str.contains("/output/"),
            "Path should NOT resolve against CDB parent (/output/build): got {resolved_str}"
        );
        assert!(
            resolved_str.contains("/lib/engine/main.cpp"),
            "Path should contain the resolved file component: got {resolved_str}"
        );
    }

    #[test]
    fn test_absolute_path_ignores_directory() {
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/output/build/compile_commands.json"),
            vec![make_entry("/src/project", "/absolute/path/to/file.cpp")],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 1);
        let s = norm(&files[0]);
        assert!(
            s.contains("/absolute/path/to/file.cpp"),
            "Absolute path should be preserved: got {s}"
        );
    }

    #[test]
    fn test_empty_directory_falls_back_to_cdb_parent() {
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/build/compile_commands.json"),
            vec![make_entry("", "src/main.cpp")],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 1);
        let s = norm(&files[0]);
        assert!(
            s.contains("/build/src/main.cpp"),
            "Empty directory should fall back to CDB parent: got {s}"
        );
    }

    #[test]
    fn test_cmake_style_same_directory_still_works() {
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/project/build/compile_commands.json"),
            vec![make_entry("/project/build", "../src/main.cpp")],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 1);
        let s = norm(&files[0]);
        // ../src/main.cpp from /project/build → /project/src/main.cpp
        assert!(
            s.contains("/project/") && s.ends_with("src/main.cpp"),
            "CMake-style resolution should still work: got {s}"
        );
    }

    #[test]
    fn test_path_mappings_use_entry_directory() {
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/output/compile_commands.json"),
            vec![make_entry("/src/project", "lib/utils.cpp")],
        );

        let (orig_to_canon, _canon_to_orig) = cdb.path_mappings().unwrap();
        let original = PathBuf::from("lib/utils.cpp");
        let canonical = orig_to_canon.get(&original).expect("mapping should exist");
        let s = norm(canonical);

        assert!(
            !s.contains("/output/"),
            "path_mappings should resolve against entry directory: got {s}"
        );
        assert!(
            s.contains("/src/project/lib/utils.cpp"),
            "path_mappings should contain correct resolved path: got {s}"
        );
    }

    #[test]
    fn test_multiple_entries_different_directories() {
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/output/compile_commands.json"),
            vec![
                make_entry("/project/moduleA", "src/a.cpp"),
                make_entry("/project/moduleB", "src/b.cpp"),
            ],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 2);

        let paths: Vec<String> = files.iter().map(|p| norm(p)).collect();
        assert!(
            paths
                .iter()
                .any(|p| p.contains("/project/moduleA/src/a.cpp")),
            "Should resolve a.cpp against moduleA: got {:?}",
            paths
        );
        assert!(
            paths
                .iter()
                .any(|p| p.contains("/project/moduleB/src/b.cpp")),
            "Should resolve b.cpp against moduleB: got {:?}",
            paths
        );
    }

    #[test]
    fn test_custom_build_system_scenario() {
        // Real-world scenario: CDB in output dir, entry directory in source tree
        let cdb = CompilationDatabase::from_entries_with_path(
            PathBuf::from("/output/_clangd/18.1.8/framework/windows/compile_commands.json"),
            vec![make_entry(
                "/project/subsystem/prog",
                "../../prog/engine/phys/physics.cpp",
            )],
        );

        let files = cdb.canonical_source_files().unwrap();
        assert_eq!(files.len(), 1);

        let s = norm(&files[0]);
        assert!(
            !s.contains("_clangd"),
            "Must NOT resolve against CDB parent: got {s}"
        );
        // ../../prog/engine/... from /project/subsystem/prog
        // → /project/prog/engine/phys/physics.cpp
        assert!(
            s.contains("/project/prog/engine/phys/physics.cpp")
                || s.contains("project/prog/engine/phys/physics.cpp"),
            "Should resolve to source tree: got {s}"
        );
    }
}

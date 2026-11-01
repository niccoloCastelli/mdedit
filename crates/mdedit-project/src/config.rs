//! `.mdedit/project.toml` (SPEC 6.2). Unknown keys are kept out of scope for
//! now; every field has a default so a partial file loads.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub git: GitSection,
    pub files: FilesSection,
    pub export: ExportSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSection {
    pub name: String,
    pub default_branch: String,
}

impl Default for ProjectSection {
    fn default() -> Self {
        Self { name: String::new(), default_branch: "main".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitSection {
    pub autocommit: bool,
    pub autocommit_debounce_secs: u64,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub remote: String,
    pub pull_on_open: bool,
}

impl Default for GitSection {
    fn default() -> Self {
        Self {
            autocommit: true,
            autocommit_debounce_secs: 60,
            author_name: None,
            author_email: None,
            remote: "origin".into(),
            pull_on_open: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesSection {
    pub large_file_threshold_mb: Option<u64>,
    pub assets_dir: String,
}

impl Default for FilesSection {
    fn default() -> Self {
        Self { large_file_threshold_mb: None, assets_dir: "assets".into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSection {
    pub default_template: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
            [project]
            name = "My notes"
            [git]
            autocommit_debounce_secs = 30
            [files]
            large_file_threshold_mb = 50
            "#,
        )
        .unwrap();
        assert_eq!(cfg.project.name, "My notes");
        assert_eq!(cfg.project.default_branch, "main");
        assert_eq!(cfg.git.autocommit_debounce_secs, 30);
        assert!(cfg.git.autocommit);
        assert_eq!(cfg.files.large_file_threshold_mb, Some(50));
    }
}

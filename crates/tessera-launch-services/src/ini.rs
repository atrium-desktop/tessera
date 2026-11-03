//! Minimal in-tree INI / Desktop-entry / index.theme parser (ADR-0208).
//!
//! Replaces the external `rust-ini` crate to eliminate transitive proc-macros,
//! Keccak hashing, and duplicate `getrandom` versions.

use std::collections::HashMap;
use std::path::Path;

/// Key-value properties within one INI section.
#[derive(Debug, Clone, Default)]
pub struct Properties {
    map: HashMap<String, String>,
}

impl Properties {
    /// Look up a property by key, returning a string slice if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|s| s.as_str())
    }

    /// Insert or update a property.
    pub fn insert(&mut self, key: String, val: String) {
        self.map.insert(key, val);
    }
}

/// A parsed INI document organized by section header.
#[derive(Debug, Clone, Default)]
pub struct Ini {
    sections: HashMap<String, Properties>,
}

impl Ini {
    /// Parse an INI document from a UTF-8 string.
    pub fn load_from_str(text: &str) -> Result<Self, String> {
        let mut sections: HashMap<String, Properties> = HashMap::new();
        let mut current_section = String::new();

        for line in text.lines() {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            // Section header: [Section Name]
            if line.starts_with('[') && line.ends_with(']') {
                current_section = line[1..line.len() - 1].trim().to_string();
                continue;
            }
            // Key-value pair
            if let Some((key, val)) = line.split_once('=') {
                sections
                    .entry(current_section.clone())
                    .or_default()
                    .insert(key.trim().to_string(), val.trim().to_string());
            }
        }
        Ok(Self { sections })
    }

    /// Load and parse an INI document from a filesystem path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::load_from_str(&content)
    }

    /// Retrieve a section by name. `Some(name)` queries `name`, `None` queries the top-level section `""`.
    pub fn section(&self, name: Option<&str>) -> Option<&Properties> {
        self.sections.get(name.unwrap_or(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_entry_section_and_properties() {
        let text = r#"
            # Comment
            [Desktop Entry]
            Type=Application
            Name=Test App
            Name[zh_CN]=测试应用
            Exec=test-app %U
            Icon=test-app
            Hidden=false
        "#;
        let ini = Ini::load_from_str(text).expect("parse valid desktop entry");
        let section = ini.section(Some("Desktop Entry")).expect("has section");
        assert_eq!(section.get("Type"), Some("Application"));
        assert_eq!(section.get("Name"), Some("Test App"));
        assert_eq!(section.get("Name[zh_CN]"), Some("测试应用"));
        assert_eq!(section.get("Hidden"), Some("false"));
    }
}

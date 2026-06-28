//! Tag engine — semantic tags from workflow signals only.
//!
//! Anti-overreach rule (docs/SPEC.md): the system operates on workflow
//! signals — apps, files, projects, sessions. The tag engine therefore
//! derives tags from path structure and extension ONLY. It never opens or
//! reads file content. Content-derived tags come from the Memory agent
//! under explicit consent scope.

/// Closed extension → (class, language) table. Extending it is a code
/// change reviewed by a human, not runtime configuration.
const EXT_TABLE: &[(&str, &str, Option<&str>)] = &[
    ("rs", "code", Some("rust")),
    ("py", "code", Some("python")),
    ("c", "code", Some("c")),
    ("h", "code", Some("c")),
    ("cpp", "code", Some("cpp")),
    ("hpp", "code", Some("cpp")),
    ("js", "code", Some("javascript")),
    ("ts", "code", Some("typescript")),
    ("sh", "code", Some("shell")),
    ("go", "code", Some("go")),
    ("md", "document", None),
    ("txt", "document", None),
    ("pdf", "document", None),
    ("odt", "document", None),
    ("png", "image", None),
    ("jpg", "image", None),
    ("jpeg", "image", None),
    ("svg", "image", None),
    ("toml", "config", None),
    ("yaml", "config", None),
    ("yml", "config", None),
    ("json", "config", None),
    ("ini", "config", None),
    ("lock", "build", None),
    ("makefile", "build", None),
];

/// Derive semantic tags for a path. Pure function of the path string.
pub fn derive_tags(path: &str) -> Vec<String> {
    let lower = path.to_lowercase();
    let mut tags = Vec::new();

    let ext = std::path::Path::new(&lower)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    let file_name = std::path::Path::new(&lower)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    for (table_ext, class, lang) in EXT_TABLE {
        if ext == *table_ext || file_name == *table_ext {
            tags.push((*class).to_string());
            if let Some(lang) = lang {
                tags.push((*lang).to_string());
            }
            break;
        }
    }

    if lower.contains("/tests/") || file_name.starts_with("test_") {
        tags.push("test".to_string());
    }
    if lower.contains("/docs/") {
        tags.push("documentation".to_string());
    }

    if let Some(domain) = derive_domain(path) {
        tags.push(format!("project:{}", domain));
    }

    tags
}

/// Derive the project domain from path structure:
/// the directory segment following "projects" (e.g.
/// `~/projects/robo-arm/motor.py` → `robo-arm`).
pub fn derive_domain(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let parts: Vec<&str> = lower
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let idx = parts.iter().position(|p| *p == "projects")?;
    parts.get(idx + 1).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_source_gets_code_and_language_tags() {
        let tags = derive_tags("~/projects/robo-arm/src/motor.rs");
        assert!(tags.contains(&"code".to_string()));
        assert!(tags.contains(&"rust".to_string()));
        assert!(tags.contains(&"project:robo-arm".to_string()));
    }

    #[test]
    fn test_files_are_tagged() {
        let tags = derive_tags("~/projects/app/tests/test_motor.py");
        assert!(tags.contains(&"test".to_string()));
        assert!(tags.contains(&"python".to_string()));
    }

    #[test]
    fn domain_requires_projects_segment() {
        assert_eq!(derive_domain("~/projects/pid-tuning/pid.py"), Some("pid-tuning".into()));
        assert_eq!(derive_domain("~/documents/letter.odt"), None);
    }

    #[test]
    fn unknown_extension_yields_no_class_tag() {
        let tags = derive_tags("~/file.xyz");
        assert!(tags.is_empty());
    }
}

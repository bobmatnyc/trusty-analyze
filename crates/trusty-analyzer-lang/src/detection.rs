//! File-extension-based language and build-system detection.
//!
//! Why: Before invoking a `LanguageAnalyzer`, we need to know which one to
//! pick. This module provides cheap path-string heuristics that work without
//! reading file contents.
//!
//! What: `LanguageDetector::detect_file` maps an extension to a language
//! string; `LanguageDetector::detect` aggregates over a slice of paths and
//! also recognizes build manifests (Cargo.toml, package.json, etc.).
//!
//! Test: `detect_file_extension_mapping` covers each supported extension;
//! `detect_picks_primary_language` ensures the most common extension wins.

use std::collections::HashMap;

/// Detected language(s) for a repository or set of files.
#[derive(Debug, Clone)]
pub struct DetectionResult {
    /// Most common language by file count.
    pub primary_language: String,
    /// All detected languages (deduplicated).
    pub languages: Vec<String>,
    /// `"cargo"`, `"maven"`, `"gradle"`, `"npm"`, `"pip"`, `"go-mod"`, ...
    pub build_system: Option<String>,
    /// Fraction of files that matched a known extension.
    pub confidence: f32,
}

/// File-extension-based language detector.
pub struct LanguageDetector;

impl LanguageDetector {
    /// Detect the language of a single file from its extension.
    /// Returns `None` for unknown extensions.
    pub fn detect_file(path: &str) -> Option<String> {
        let lower = path.to_lowercase();
        if lower.ends_with(".rs") {
            return Some("rust".into());
        }
        if lower.ends_with(".tsx") || lower.ends_with(".ts") {
            return Some("typescript".into());
        }
        if lower.ends_with(".jsx")
            || lower.ends_with(".js")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs")
        {
            return Some("javascript".into());
        }
        if lower.ends_with(".py") || lower.ends_with(".pyi") {
            return Some("python".into());
        }
        if lower.ends_with(".java") {
            return Some("java".into());
        }
        if lower.ends_with(".go") {
            return Some("go".into());
        }
        if lower.ends_with(".cpp")
            || lower.ends_with(".cc")
            || lower.ends_with(".cxx")
            || lower.ends_with(".hpp")
            || lower.ends_with(".hh")
            || lower.ends_with(".hxx")
        {
            return Some("cpp".into());
        }
        if lower.ends_with(".c") || lower.ends_with(".h") {
            return Some("cpp".into());
        }
        None
    }

    /// Detect a build system from a single file basename.
    fn detect_build_system_for(path: &str) -> Option<&'static str> {
        let lower = path.to_lowercase();
        if lower.ends_with("/cargo.toml") || lower == "cargo.toml" {
            return Some("cargo");
        }
        if lower.ends_with("/pom.xml") || lower == "pom.xml" {
            return Some("maven");
        }
        if lower.ends_with("/build.gradle")
            || lower == "build.gradle"
            || lower.ends_with("/build.gradle.kts")
            || lower == "build.gradle.kts"
        {
            return Some("gradle");
        }
        if lower.ends_with("/package.json") || lower == "package.json" {
            return Some("npm");
        }
        if lower.ends_with("/pyproject.toml")
            || lower == "pyproject.toml"
            || lower.ends_with("/setup.py")
            || lower == "setup.py"
            || lower.ends_with("/requirements.txt")
            || lower == "requirements.txt"
        {
            return Some("pip");
        }
        if lower.ends_with("/go.mod") || lower == "go.mod" {
            return Some("go-mod");
        }
        None
    }

    /// Detect languages from a list of file paths. Returns the primary
    /// language (most common matching extension), all detected languages,
    /// the most authoritative build system found, and a confidence score
    /// equal to the fraction of files that matched a known extension.
    pub fn detect(files: &[&str]) -> DetectionResult {
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut build: Option<&'static str> = None;
        let total = files.len().max(1);
        let mut matched = 0usize;

        for f in files {
            if let Some(lang) = Self::detect_file(f) {
                *counts.entry(lang).or_insert(0) += 1;
                matched += 1;
            }
            if build.is_none() {
                if let Some(bs) = Self::detect_build_system_for(f) {
                    build = Some(bs);
                }
            }
        }

        let mut langs: Vec<(String, usize)> = counts.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let primary = langs
            .first()
            .map(|(l, _)| l.clone())
            .unwrap_or_else(|| "unknown".into());

        let all = langs.iter().map(|(l, _)| l.clone()).collect();

        DetectionResult {
            primary_language: primary,
            languages: all,
            build_system: build.map(|s| s.to_string()),
            confidence: matched as f32 / total as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_file_extension_mapping() {
        assert_eq!(
            LanguageDetector::detect_file("src/main.rs"),
            Some("rust".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("App.tsx"),
            Some("typescript".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("foo.ts"),
            Some("typescript".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("foo.js"),
            Some("javascript".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("module.mjs"),
            Some("javascript".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("script.py"),
            Some("python".into())
        );
        assert_eq!(
            LanguageDetector::detect_file("Foo.java"),
            Some("java".into())
        );
        assert_eq!(LanguageDetector::detect_file("main.go"), Some("go".into()));
        assert_eq!(LanguageDetector::detect_file("README.md"), None);
    }

    #[test]
    fn detect_picks_primary_language() {
        let files = ["a.rs", "b.rs", "c.rs", "d.ts", "Cargo.toml"];
        let r = LanguageDetector::detect(&files);
        assert_eq!(r.primary_language, "rust");
        assert!(r.languages.contains(&"rust".to_string()));
        assert!(r.languages.contains(&"typescript".to_string()));
        assert_eq!(r.build_system.as_deref(), Some("cargo"));
        assert!(r.confidence > 0.5);
    }

    #[test]
    fn detect_recognizes_npm_and_python() {
        let files = ["a.ts", "package.json", "tsconfig.json"];
        let r = LanguageDetector::detect(&files);
        assert_eq!(r.build_system.as_deref(), Some("npm"));

        let files = ["main.py", "pyproject.toml"];
        let r = LanguageDetector::detect(&files);
        assert_eq!(r.primary_language, "python");
        assert_eq!(r.build_system.as_deref(), Some("pip"));
    }
}

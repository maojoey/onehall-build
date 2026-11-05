//! Lexical matching of declared relative FILE paths, not filesystem resolution
//! or authorization. Execution adapters must first prove real-path containment
//! and apply the member's trusted identity, native policy and approval gates.
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ScopeFault {
    pub code: &'static str,
    pub field: &'static str,
    pub index: Option<usize>,
}

pub struct CompiledMemberScope {
    read: GlobSet,
    write: GlobSet,
}

fn canonical(raw: &str, pattern: bool) -> Option<String> {
    if raw.is_empty()
        || raw.trim() != raw
        || raw.chars().any(char::is_control)
        || raw.starts_with(['/', '~'])
        || raw.contains(['\\', ':'])
        || (pattern && raw.starts_with('!'))
        || raw.split('/').any(|part| part == "..")
    {
        return None;
    }
    let path = raw
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    (!path.is_empty()).then_some(path)
}

fn compile(patterns: &[String], field: &'static str) -> Result<GlobSet, ScopeFault> {
    let fault = |index| ScopeFault {
        code: "invalid_path_rule",
        field,
        index: Some(index),
    };
    if patterns.len() > 64 {
        return Err(ScopeFault {
            code: "invalid_path_rule",
            field,
            index: None,
        });
    }
    let mut builder = GlobSetBuilder::new();
    for (index, raw) in patterns.iter().enumerate() {
        if raw.chars().count() > 1024 {
            return Err(fault(index));
        }
        let path = canonical(raw, true).ok_or_else(|| fault(index))?;
        let glob = GlobBuilder::new(&path)
            .literal_separator(true)
            .backslash_escape(false)
            .case_insensitive(false)
            .build()
            .map_err(|_| fault(index))?;
        builder.add(glob);
    }
    builder.build().map_err(|_| ScopeFault {
        code: "invalid_path_rule",
        field,
        index: None,
    })
}

impl CompiledMemberScope {
    pub fn compile(read: &[String], write: &[String]) -> Result<Self, ScopeFault> {
        Ok(Self {
            read: compile(read, "scope.read")?,
            write: compile(write, "scope.write")?,
        })
    }
    /// Input is an already-contained relative file path. These matches grant
    /// no operation and do not inspect symlinks, directories or the filesystem.
    pub fn matches_declared_read_path(&self, relative: &str) -> bool {
        canonical(relative, false).is_some_and(|path| self.read.is_match(path))
    }
    pub fn matches_declared_write_path(&self, relative: &str) -> bool {
        canonical(relative, false).is_some_and(|path| self.write.is_match(path))
    }
    pub fn rule_counts(&self) -> (usize, usize) {
        (self.read.len(), self.write.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rules(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_match_relative_files_but_never_roots_or_outside_spellings() {
        let scope = CompiledMemberScope::compile(&rules(&["**"]), &[]).unwrap();
        assert!(scope.matches_declared_read_path("reports/a.md"));
        assert!(scope.matches_declared_read_path(".onehall/agent.ui.yaml"));
        assert!(!scope.matches_declared_write_path("reports/a.md"));
        for path in [
            "",
            ".",
            "./",
            "..",
            "../secret",
            "a/../b",
            "/etc/file",
            "~/file",
            "C:/file",
            "file://x",
            "a\\b",
            "a\0b",
        ] {
            assert!(!scope.matches_declared_read_path(path));
            assert!(!scope.matches_declared_write_path(path));
        }
    }
    #[test]
    fn stars_do_not_cross_directories_but_recursive_rules_do() {
        let scope =
            CompiledMemberScope::compile(&rules(&["reports/*.md"]), &rules(&["reports/out/**"]))
                .unwrap();
        assert!(scope.matches_declared_read_path("./reports//a.md"));
        assert!(!scope.matches_declared_read_path("reports/sub/a.md"));
        assert!(!scope.matches_declared_write_path("reports/output/a.md"));
        assert!(scope.matches_declared_write_path("reports/out/sub/a.md"));
        assert_eq!(scope.rule_counts(), (1, 1));
    }
    #[test]
    fn braces_and_character_classes_are_compiled_without_scanning_files() {
        let scope =
            CompiledMemberScope::compile(&rules(&["src/*.{rs,ts}", "docs/[ab]?.md"]), &[]).unwrap();
        assert!(scope.matches_declared_read_path("src/main.rs"));
        assert!(scope.matches_declared_read_path("docs/a1.md"));
        assert!(!scope.matches_declared_read_path("docs/c1.md"));
    }
    #[test]
    fn bad_rules_fail_closed_without_including_the_pattern_in_the_fault() {
        for rule in [
            "../secret",
            "/secret",
            "C:/secret",
            "!secret",
            "bad[",
            "a\\b",
            "a/../b",
            ".",
        ] {
            let fault = CompiledMemberScope::compile(&rules(&[rule]), &[])
                .err()
                .unwrap();
            assert_eq!(fault.field, "scope.read");
            assert_eq!(fault.code, "invalid_path_rule");
            assert_eq!(fault.index, Some(0));
        }
    }
}

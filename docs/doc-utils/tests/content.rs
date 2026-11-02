use std::path::Path;

#[test]
fn committed_fragments_include_version_changelog_and_all_benchmarks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fragments = doc_utils::fragments(&root).unwrap();
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(
        fragments["{{ badness-version }}"],
        manifest["package"]["version"].as_str().unwrap()
    );
    assert!(fragments["{{ changelog }}"].contains("releases/tag/v0.1.0"));
    for marker in [
        "benchmark-results",
        "benchmark-project-results",
        "lint-benchmark-results",
        "lsp-benchmark-results",
        "memory-benchmark-results",
    ] {
        let content = &fragments[&format!("{{{{ {marker} }}}}")];
        assert!(content.contains("data-bench=\""), "{marker}");
        assert!(
            content.contains("<table>") || content.contains("| Server |"),
            "{marker}"
        );
        assert!(!content.contains("<script"), "{marker}");
    }
}

#[test]
fn absent_benchmarks_produce_readable_fallbacks() {
    let root = std::env::temp_dir().join(format!("badness-doc-fragments-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nversion = '1.2.3'\n").unwrap();
    std::fs::write(root.join("CHANGELOG.md"), "# Changelog\n\n## 1.2.3\n").unwrap();
    let fragments = doc_utils::fragments(&root).unwrap();
    assert_eq!(fragments["{{ badness-version }}"], "1.2.3");
    assert!(fragments["{{ benchmark-results }}"].contains("run `task bench`"));
    assert!(fragments["{{ memory-benchmark-results }}"].contains("run `task bench:lsp`"));
    std::fs::remove_dir_all(root).unwrap();
}

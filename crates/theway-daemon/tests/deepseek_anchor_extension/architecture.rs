use std::path::{Path, PathBuf};

fn source_files(root: &Path, output: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, output);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("rs")
        ) {
            output.push(path);
        }
    }
}

#[test]
fn core_and_provider_sources_have_no_anchor_extension_behavior() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let roots = [
        repository.join("crates/theway-core/src"),
        repository.join("crates/theway-llm-provider/src"),
    ];
    let mut files = Vec::new();
    for root in roots {
        source_files(&root, &mut files);
    }

    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for marker in ["deepseek-anchor", "anchor.phase", "anchor-promotion-v1"] {
            assert!(
                !source.to_ascii_lowercase().contains(marker),
                "{} contains reference-extension behavior marker {marker}",
                file.display()
            );
        }
    }
}

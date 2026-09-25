#![expect(
    clippy::expect_used,
    reason = "source inventory regression should fail loudly when required files move or disappear"
)]

use std::fs;
use std::path::PathBuf;

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(relative)
}

fn read_repo_file(relative: &str) -> String {
    fs::read_to_string(repo_path(relative)).expect("repo inventory file should be readable")
}

fn count_lines_with_prefix(contents: &str, prefix: &str) -> usize {
    contents.lines().filter(|line| line.trim_start().starts_with(prefix)).count()
}

#[test]
fn root_wit_defines_the_pinned_inference_world() {
    let worlds = read_repo_file("wit/worlds.wit");
    assert_eq!(
        count_lines_with_prefix(&worlds, "world "),
        4,
        "the active WIT package must expose exactly four worlds"
    );
    assert!(worlds.contains("world inference-node {"));
    for interface in ["tensor", "graph", "inference", "errors"] {
        assert!(
            worlds.contains(&format!("import wasi:nn/{interface}@0.2.0-rc-2024-10-28;")),
            "inference-node must import the pinned wasi-nn {interface} interface"
        );
    }

    let wasi_nn = read_repo_file("wit/deps/wasi-nn/wasi-nn.wit");
    assert!(wasi_nn.starts_with("package wasi:nn@0.2.0-rc-2024-10-28;"));
}

#[test]
fn host_bindings_include_inference_without_changing_ordinary_worlds() {
    let bindings = read_repo_file("crates/wafer-core/src/engine/bindings.rs");
    assert_eq!(
        count_lines_with_prefix(&bindings, "world: \""),
        4,
        "bindgen must target the four active worlds"
    );
    assert!(bindings.contains("world: \"inference-node\""));
    assert!(bindings.contains("pub(crate) mod inference_node"));

    for world in ["transform-node", "filter-node", "router-node"] {
        assert!(bindings.contains(&format!("world: \"{world}\"")));
    }
}

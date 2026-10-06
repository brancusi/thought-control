//! VERSION and the workspace version (`thc --version`) agree.

#[test]
fn version_file_matches_the_workspace_version() {
    let file = include_str!("../../../VERSION").trim();
    assert_eq!(file, env!("CARGO_PKG_VERSION"), "bump VERSION and [workspace.package] version together");
}

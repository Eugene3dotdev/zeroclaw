//! Fork: `zrc` is a `zrc -> zeroclaw` symlink, not a second compiled binary.
//!
//! The release archive carries one CLI binary, and install.sh links `zrc`
//! beside it. The updater side (replacing the target while keeping the link)
//! is covered by the `commands::update` unit tests.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The body of a top-level `name() { … }` function in install.sh, the same
/// extraction the installer's shell tests use, so nothing else in the
/// installer runs.
fn install_sh_function(name: &str) -> String {
    let install = std::fs::read_to_string(repo_root().join("install.sh")).expect("install.sh");
    let header = format!("{name}() {{\n");
    let start = install
        .find(&header)
        .unwrap_or_else(|| panic!("install.sh defines {name}()"));
    let body = &install[start..];
    let end = body
        .find("\n}\n")
        .unwrap_or_else(|| panic!("{name}() has a closing brace"));
    body[..end + 3].to_string()
}

fn link_zrc_alias(bin_dir: &Path) {
    let script = format!(
        "warn() {{ printf '%s\\n' \"$*\" >&2; }}\n{}\nlink_zrc_alias \"$1\"\n",
        install_sh_function("link_zrc_alias")
    );
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(bin_dir)
        .status()
        .expect("sh runs");
    assert!(status.success(), "link_zrc_alias exits 0");
}

fn assert_zrc_links_to_zeroclaw(bin_dir: &Path) {
    let alias = bin_dir.join("zrc");
    let metadata = std::fs::symlink_metadata(&alias).expect("zrc exists");
    assert!(metadata.file_type().is_symlink(), "zrc must be a symlink");
    assert_eq!(std::fs::read_link(&alias).unwrap(), Path::new("zeroclaw"));
    assert_eq!(
        std::fs::read(&alias).unwrap(),
        std::fs::read(bin_dir.join("zeroclaw")).unwrap()
    );
}

#[test]
fn install_links_zrc_to_zeroclaw_and_replaces_a_legacy_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path();
    std::fs::write(bin.join("zeroclaw"), b"the one cli binary").unwrap();
    std::fs::write(bin.join("zrc"), b"an older release's copy").unwrap();

    link_zrc_alias(bin);
    assert_zrc_links_to_zeroclaw(bin);

    // Re-running the installer keeps the same link.
    link_zrc_alias(bin);
    assert_zrc_links_to_zeroclaw(bin);
}

#[test]
fn install_makes_no_zrc_without_zeroclaw_and_leaves_a_directory_alone() {
    let tmp = tempfile::tempdir().unwrap();
    link_zrc_alias(tmp.path());
    assert!(std::fs::symlink_metadata(tmp.path().join("zrc")).is_err());

    std::fs::write(tmp.path().join("zeroclaw"), b"cli").unwrap();
    std::fs::create_dir(tmp.path().join("zrc")).unwrap();
    link_zrc_alias(tmp.path());
    assert!(
        tmp.path().join("zrc").is_dir(),
        "a directory is not replaced"
    );
}

#[test]
fn release_archive_carries_one_cli_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let release = tmp.path().join("target/fixture-target/release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::write(release.join("zeroclaw"), b"cli").unwrap();
    std::fs::write(release.join("zerocode"), b"tui").unwrap();
    // A stale `zrc` left in a build directory must not be packaged.
    std::fs::write(release.join("zrc"), b"stale second binary").unwrap();
    std::fs::create_dir_all(tmp.path().join("web/dist")).unwrap();
    std::fs::write(tmp.path().join("web/dist/index.html"), b"<html></html>").unwrap();

    let status = Command::new("bash")
        .arg(repo_root().join("scripts/ci/package_release_archive.sh"))
        .args(["fixture-target", "zeroclaw", "tar.gz", "Linux"])
        .current_dir(tmp.path())
        .status()
        .expect("bash runs the packager");
    assert!(status.success(), "the packager succeeds");

    let listing = Command::new("tar")
        .args(["tzf", "zeroclaw-fixture-target.tar.gz"])
        .current_dir(tmp.path())
        .output()
        .expect("tar lists the archive");
    assert!(listing.status.success());
    let members: Vec<String> = String::from_utf8(listing.stdout)
        .unwrap()
        .lines()
        .map(|line| line.trim_end_matches('/').to_string())
        .collect();
    assert!(
        members.iter().any(|member| member == "zeroclaw"),
        "{members:?}"
    );
    assert!(
        members.iter().any(|member| member == "zerocode"),
        "{members:?}"
    );
    assert!(
        !members.iter().any(|member| member == "zrc"),
        "the archive must not carry a second CLI binary: {members:?}"
    );
}

#[test]
fn the_cli_is_compiled_once() {
    let manifest = std::fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    let manifest: toml::Table = manifest.parse().unwrap();
    let bins: Vec<(&str, &str)> = manifest["bin"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bin| (bin["name"].as_str().unwrap(), bin["path"].as_str().unwrap()))
        .collect();
    assert!(
        !bins.iter().any(|(name, _)| *name == "zrc"),
        "zrc is a symlink, not a [[bin]]: {bins:?}"
    );
    let from_main: Vec<_> = bins
        .iter()
        .filter(|(_, path)| *path == "src/main.rs")
        .collect();
    assert_eq!(
        from_main.len(),
        1,
        "exactly one bin builds src/main.rs: {bins:?}"
    );
}

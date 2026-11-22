// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regression coverage for validated filesystem asset access.

use auro::assets::{AssetAccessError, FileAssets};
use auro::gpui::AssetSource;

#[test]
fn ordinary_in_root_loads_lists_and_missing_files_still_work() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("assets");
    std::fs::create_dir(&root).expect("asset root");
    std::fs::create_dir(root.join("icons")).expect("icons directory");
    std::fs::write(root.join("icons/logo.svg"), b"<svg/>").expect("logo");
    std::fs::write(root.join("icons/nested.svg"), b"nested").expect("nested");
    std::fs::write(root.join("note.txt"), b"note").expect("note");

    let assets = FileAssets::new(root.clone());
    assert_eq!(assets.root(), root.as_path());
    assert_eq!(
        assets.load("icons/logo.svg").expect("load").as_deref(),
        Some(&b"<svg/>"[..])
    );
    assert_eq!(
        assets.load("note.txt").expect("load").as_deref(),
        Some(&b"note"[..])
    );
    assert!(assets.load("missing.svg").expect("load").is_none());

    #[cfg(unix)]
    {
        let names: Vec<String> = assets
            .list("icons")
            .expect("list")
            .into_iter()
            .map(|name| name.to_string())
            .collect();
        assert_eq!(names, ["logo.svg", "nested.svg"]);
        let names: Vec<String> = assets
            .list("")
            .expect("list root")
            .into_iter()
            .map(|name| name.to_string())
            .collect();
        assert_eq!(names, ["icons", "note.txt"]);
    }

    #[cfg(windows)]
    {
        // std cannot enumerate directory entries from a verified handle on Windows.
        let error = assets.list("icons").expect_err("unsupported on Windows");
        assert!(
            matches!(
                error.downcast_ref::<AssetAccessError>(),
                Some(AssetAccessError::Unsupported { .. })
            ),
            "expected typed unsupported error, got {error:?}"
        );
    }

    for path in [
        "../note.txt",
        "/etc/passwd",
        "icons/../../note.txt",
        "a\\note.txt",
    ] {
        assert!(assets.load(path).is_err(), "{path} must be rejected");
    }
}

#[cfg(unix)]
#[test]
fn symlink_swapped_after_check_never_returns_outside_content() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("assets");
    std::fs::create_dir(&root).expect("root");
    let inside = root.join("inside.svg");
    std::fs::write(&inside, b"inside").expect("inside");
    let outside = temp.path().join("secret.svg");
    std::fs::write(&outside, b"outside").expect("outside");

    let link = root.join("asset.svg");
    symlink(&inside, &link).expect("initial link");
    // The check a path-based loader would perform sees a target inside the root. Swapping the link
    // after that check is done here explicitly, so the regression needs no timing or retries.
    assert_eq!(
        std::fs::canonicalize(&link).expect("canonicalize before swap"),
        std::fs::canonicalize(&inside).expect("canonicalize inside")
    );
    std::fs::remove_file(&link).expect("unlink");
    symlink(&outside, &link).expect("swapped link");

    let assets = FileAssets::new(root.clone());
    match assets.load("asset.svg") {
        Ok(Some(bytes)) => panic!("out-of-root content returned: {:?}", bytes.as_ref()),
        Ok(None) => panic!("a swapped symlink must not look like a missing file"),
        Err(error) => assert!(
            matches!(
                error.downcast_ref::<AssetAccessError>(),
                Some(AssetAccessError::Link { .. })
            ),
            "expected typed link error, got {error:?}"
        ),
    }
}

#[cfg(unix)]
#[test]
fn symlinked_parent_component_is_rejected() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("assets");
    std::fs::create_dir(&root).expect("asset root");
    std::fs::create_dir(root.join("real")).expect("real directory");
    std::fs::write(root.join("real/file.svg"), b"inside").expect("file");
    symlink(root.join("real"), root.join("alias")).expect("directory link");

    let assets = FileAssets::new(root.clone());
    let error = assets
        .load("alias/file.svg")
        .expect_err("symlinked parent must be rejected");
    assert!(
        matches!(
            error.downcast_ref::<AssetAccessError>(),
            Some(AssetAccessError::Link { .. })
        ),
        "expected typed link error, got {error:?}"
    );
}

use super::*;
use tempfile::TempDir;

fn source(dir: &TempDir) -> PathBuf {
    let source = dir.path().join("Документы");
    std::fs::create_dir(&source).unwrap();
    source
}
fn fixture_bundle(
    name: &str,
    entries: &[(u8, &str, u64, &[u8])],
    declared_total: Option<u64>,
) -> NamedTempFile {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    bytes.extend_from_slice(
        &declared_total
            .unwrap_or_else(|| entries.iter().map(|e| e.2).sum())
            .to_be_bytes(),
    );
    for (kind, path, size, payload) in entries {
        bytes.push(*kind);
        bytes.extend_from_slice(&(path.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(payload);
    }
    let hash = Sha256::digest(&bytes);
    bytes.extend_from_slice(&hash);
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file
}
fn assert_clean_rejection(bundle: &Path) {
    let parent = tempfile::tempdir().unwrap();
    assert!(prepare_unpack(bundle, parent.path(), &CancellationToken::default()).is_err());
    assert_eq!(
        std::fs::read_dir(parent.path()).unwrap().count(),
        0,
        "partial extraction survived"
    );
}

#[test]
fn nested_unicode_files_and_empty_directories_roundtrip() {
    let input = tempfile::tempdir().unwrap();
    let source = source(&input);
    std::fs::create_dir(source.join("пусто")).unwrap();
    std::fs::create_dir(source.join("资料")).unwrap();
    std::fs::write(source.join("资料/Привет.txt"), "Привет 🌍").unwrap();
    std::fs::write(source.join("нулевой файл"), b"").unwrap();
    let prepared = pack(&source).unwrap();
    assert_eq!(prepared.name, "Документы");
    assert_eq!(prepared.entries, 4);
    assert!(prepared.path().to_str().unwrap().ends_with(".assi-folder"));
    let output = tempfile::tempdir().unwrap();
    let extraction = prepare_unpack(
        prepared.path(),
        output.path(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(!output.path().join("Документы").exists());
    let extracted = extraction.publish().unwrap();
    assert_eq!(
        extracted.canonicalize().unwrap(),
        output.path().join("Документы").canonicalize().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(extracted.join("资料/Привет.txt")).unwrap(),
        "Привет 🌍"
    );
    assert!(extracted.join("пусто").is_dir());
    assert_eq!(
        std::fs::metadata(extracted.join("нулевой файл"))
            .unwrap()
            .len(),
        0
    );
}
#[test]
fn empty_root_folder_roundtrips() {
    let input = tempfile::tempdir().unwrap();
    let source = source(&input);
    let bundle = pack(&source).unwrap();
    assert_eq!(bundle.entries, 0);
    let output = tempfile::tempdir().unwrap();
    let extracted = unpack(bundle.path(), output.path()).unwrap();
    assert!(extracted.is_dir());
    assert_eq!(std::fs::read_dir(extracted).unwrap().count(), 0);
}
#[test]
fn existing_file_or_directory_is_never_overwritten_even_if_created_after_prepare() {
    let bundle = fixture_bundle("root", &[(2, "file", 3, b"new")], None);
    let output = tempfile::tempdir().unwrap();
    let extraction =
        prepare_unpack(bundle.path(), output.path(), &CancellationToken::default()).unwrap();
    std::fs::create_dir(output.path().join("root")).unwrap();
    std::fs::write(output.path().join("root/keep"), b"old").unwrap();
    std::fs::write(output.path().join("root (2)"), b"existing file").unwrap();
    let result = extraction.publish().unwrap();
    assert_eq!(
        result.canonicalize().unwrap(),
        output.path().join("root (3)").canonicalize().unwrap()
    );
    assert_eq!(std::fs::read(result.join("file")).unwrap(), b"new");
    assert_eq!(
        std::fs::read(output.path().join("root/keep")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(output.path().join("root (2)")).unwrap(),
        b"existing file"
    );
}
#[test]
fn cancellation_before_pack_and_before_publish_cleans_private_files() {
    let input = tempfile::tempdir().unwrap();
    let source = source(&input);
    let cancel = CancellationToken::default();
    cancel.cancel();
    assert!(matches!(
        pack_with_cancel(&source, &cancel),
        Err(FolderError::Cancelled)
    ));
    let bundle = fixture_bundle("root", &[(2, "file", 1, b"x")], None);
    let output = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::default();
    let prepared = prepare_unpack(bundle.path(), output.path(), &cancel).unwrap();
    cancel.cancel();
    assert!(matches!(prepared.publish(), Err(FolderError::Cancelled)));
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
}
#[test]
fn dropping_unpublished_extraction_or_bundle_removes_temporary_data() {
    let input = tempfile::tempdir().unwrap();
    let source = source(&input);
    let bundle = pack(&source).unwrap();
    let path = bundle.path().to_owned();
    let output = tempfile::tempdir().unwrap();
    let extraction = prepare_unpack(&path, output.path(), &CancellationToken::default()).unwrap();
    drop(extraction);
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    drop(bundle);
    assert!(!path.exists());
}

#[test]
fn parent_rename_keeps_publication_and_cleanup_bound_to_original_directory() {
    let base = tempfile::tempdir().unwrap();
    let selected = base.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    let bundle = fixture_bundle("root", &[(2, "file", 3, b"new")], None);
    let prepared = prepare_unpack(bundle.path(), &selected, &CancellationToken::default()).unwrap();
    let staging = prepared.temporary.name.clone();
    let moved = base.path().join("moved");
    std::fs::rename(&selected, &moved).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(selected.join(&staging)).unwrap();
    std::fs::write(selected.join(&staging).join("foreign"), b"keep").unwrap();
    let result = prepared.publish().unwrap();
    assert_eq!(std::fs::read(result.join("file")).unwrap(), b"new");
    assert_eq!(std::fs::read(moved.join("root/file")).unwrap(), b"new");
    assert_eq!(
        std::fs::read(selected.join(&staging).join("foreign")).unwrap(),
        b"keep"
    );

    let prepared = prepare_unpack(bundle.path(), &selected, &CancellationToken::default()).unwrap();
    let staging = prepared.temporary.name.clone();
    let moved_again = base.path().join("moved-again");
    std::fs::rename(&selected, &moved_again).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(selected.join(&staging)).unwrap();
    std::fs::write(selected.join(&staging).join("foreign"), b"keep").unwrap();
    drop(prepared);
    assert!(!moved_again.join(&staging).exists());
    assert_eq!(
        std::fs::read(selected.join(&staging).join("foreign")).unwrap(),
        b"keep"
    );
}
#[test]
fn traversal_absolute_ads_reserved_and_control_names_are_rejected() {
    for path in [
        "../escape",
        "/absolute",
        "a/../../escape",
        "a\\b",
        "C:secret",
        "file:stream",
        "CON",
        "nul.txt",
        "COM1.log",
        "LPT²",
        "a/",
        "a//b",
        "a.",
        "a ",
        "a\u{202e}txt",
        "a\0b",
    ] {
        let file = fixture_bundle("root", &[(2, path, 1, b"x")], None);
        assert_clean_rejection(file.path());
    }
    for name in ["..", "CON", "/abs"] {
        let file = fixture_bundle(name, &[], None);
        assert_clean_rejection(file.path());
    }
}
#[test]
fn case_unicode_alias_duplicate_and_file_parent_collisions_are_rejected() {
    for entries in [
        vec![(2, "A", 1, b"x".as_slice()), (2, "a", 1, b"y".as_slice())],
        vec![
            (2, "é", 1, b"x".as_slice()),
            (2, "e\u{301}", 1, b"y".as_slice()),
        ],
        vec![
            (2, "same", 1, b"x".as_slice()),
            (2, "same", 1, b"y".as_slice()),
        ],
        vec![
            (2, "a", 1, b"x".as_slice()),
            (2, "a/file", 1, b"y".as_slice()),
        ],
        vec![
            (1, "Folder", 0, b"".as_slice()),
            (2, "folder/file", 1, b"y".as_slice()),
        ],
    ] {
        let bundle = fixture_bundle("root", &entries, None);
        assert_clean_rejection(bundle.path());
    }
}
#[test]
fn checksum_truncation_trailing_data_unknown_kinds_and_size_mismatch_rejected() {
    for entries in [
        vec![(3, "symlink", 0, b"".as_slice())],
        vec![(1, "dir", 1, b"x".as_slice())],
        vec![(2, "file", 5, b"short".as_slice())],
    ] {
        let file = fixture_bundle("root", &entries, Some(1));
        assert_clean_rejection(file.path());
    }
    let file = fixture_bundle("root", &[(2, "file", 1, b"x")], None);
    let original = std::fs::read(file.path()).unwrap();
    for bytes in [
        original[..original.len() - 1].to_vec(),
        {
            let mut b = original.clone();
            b.push(0);
            b
        },
        {
            let mut b = original.clone();
            let index = b.len() - 1;
            b[index] ^= 1;
            b
        },
    ] {
        std::fs::write(file.path(), bytes).unwrap();
        assert_clean_rejection(file.path());
    }
}
#[test]
fn declared_limits_are_rejected_before_payload_or_destination_creation() {
    let mut file = fixture_bundle("root", &[], Some(MAX_BUNDLE_BYTES + 1));
    assert_clean_rejection(file.path());
    file.as_file_mut().set_len(MAX_BUNDLE_BYTES + 1).unwrap();
    assert_clean_rejection(file.path());
    let long = (0..MAX_DEPTH + 1)
        .map(|_| "a")
        .collect::<Vec<_>>()
        .join("/");
    assert_eq!(relative(&long), Err(FolderError::Limit));
    assert!(component(&"a".repeat(256)).is_err());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&4u16.to_be_bytes());
    bytes.extend_from_slice(b"root");
    bytes.extend_from_slice(&(MAX_ENTRIES + 1).to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&[0; 32]);
    std::fs::write(file.path(), bytes).unwrap();
    assert_clean_rejection(file.path());
}
#[cfg(unix)]
#[test]
fn sender_rejects_symlinked_root_descendants_hardlinks_and_fifo() {
    use std::os::unix::fs::symlink;
    let input = tempfile::tempdir().unwrap();
    let source = source(&input);
    let elsewhere = input.path().join("outside");
    std::fs::write(&elsewhere, b"private").unwrap();
    symlink(&source, input.path().join("link-root")).unwrap();
    assert!(matches!(
        pack(&input.path().join("link-root")),
        Err(FolderError::UnsupportedEntry)
    ));
    symlink(&elsewhere, source.join("link")).unwrap();
    assert!(matches!(pack(&source), Err(FolderError::UnsupportedEntry)));
    std::fs::remove_file(source.join("link")).unwrap();
    std::fs::hard_link(&elsewhere, source.join("hardlink")).unwrap();
    assert!(matches!(pack(&source), Err(FolderError::UnsupportedEntry)));
    std::fs::remove_file(source.join("hardlink")).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let pipe = std::ffi::CString::new(source.join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: `pipe` is NUL-terminated and lives through this local test call.
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    assert!(matches!(pack(&source), Err(FolderError::UnsupportedEntry)));
}

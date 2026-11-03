# assi-folders

Streaming folder transfer container with an explicit, bounded extraction step.
The format is internal to Assi; it is not ZIP and does not decompress data.

## Integration

- `pack(&Path)` / `pack_with_cancel(&Path, &CancellationToken)` return
  `PreparedFolder`. Keep this guard alive while the existing file-transfer engine
  reads `path()`. Dropping it deletes the prepared bundle. The generated filename
  ends in `.assi-folder`; `name`, `entries`, and `total_bytes` describe its contents.
- The receiver first accepts the bundle using the existing authenticated,
  integrity-checked file-transfer channel. Extract only after explicit user
  selection of the destination parent. An extension alone is not authorization.
- `prepare_unpack(bundle, parent, token)` streams into a separate staging directory,
  verifies all paths and the container checksum, and returns `PreparedExtraction`.
  It does not create the final destination name.
- Reacquire the session/lifecycle lock, check the request and consent are still
  valid, then call `publish()`. Publication uses an atomic no-replace rename. An
  existing destination causes selection of `name (2)`, etc.; never overwrite it.
- Drop an unpublished extraction on failure/cancellation. Creation, traversal,
  cleanup and publication are bound to directory handles; replacing the original
  parent pathname cannot redirect cleanup into another folder.
- `unpack` is a convenience wrapper for callers that do not need a separate
  authorization boundary. Native session integration should use the two phases.

Cancellation is checked during scanning, before each entry, every 64 KiB copied,
and before publication. The native owner must serialize its final consent check
and `publish()` with session cancellation; the crate does not own session state.

## Format v1 and limits

All integers are big-endian. Header: `ASSIFOLD` + version byte `1`, root-name byte
length `u16`, UTF-8 root name, entry count `u32`, total payload bytes `u64`.
Each entry: kind `u8` (`1` directory, `2` regular file), path byte length `u16`,
payload size `u64`, UTF-8 relative path, and exactly that many payload bytes.
Directories have size zero and precede descendants. A final 32-byte SHA-256
covers the complete header and entries. Trailing bytes are rejected.

- Maximum bundle size **1 GiB including metadata**, 10,000 entries, depth 32.
- Relative path maximum 4,096 UTF-8 bytes; each component at most 255 bytes.
- Explicit empty directories and zero-byte files are retained.
- Names retain Unicode. Canonical Unicode/case aliases, duplicate entries and
  ambiguous parent spelling are rejected for cross-platform portability.
- Absolute paths, `..`, backslashes, Windows ADS/reserved names, trailing spaces
  or dots, control/bidirectional formatting characters are rejected.
- Sender traversal uses `cap-std` handles and no-follow opens; symbolic links,
  multiply-linked files, FIFO/socket/device nodes are rejected. File identity,
  size and modification metadata are checked before and after reading.
- Extraction does not restore executable permissions, ownership, ACLs, timestamps,
  links or other special attributes. No archive entry can create a link.

macOS/Linux publish with `renameat_with(NOREPLACE)`. Windows uses
`SetFileInformationByHandle(FILE_RENAME_INFO)` with `ReplaceIfExists=false` and
a captured `RootDirectory` handle, including a handle-relative source open.
Unsupported filesystem rename semantics cause an error and cleanup, never an
overwrite fallback. Returned paths are resolved from the selected directory handle
so an external rename of that directory does not return its obsolete pathname.

## Verification

Tests cover nested Unicode/empty-directory round trips, checksum/truncation/size
errors, traversal and name aliases, hard/symbolic links and FIFO rejection,
cancellation, staged cleanup, destination collisions and parent replacement.
They exercise real temporary files; Windows cross-compilation is separate from
physical Windows runtime verification.

Primary references:
[cap-std Dir](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html),
[cap-fs-ext](https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/),
[Microsoft FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info).

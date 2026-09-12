# Reliability and performance checks

Implemented September 12, 2026:

- Document and preference writes stage data in the destination directory, sync it,
  and atomically replace the destination. A Unix directory sync error after
  replacement is a committed-save durability warning: the editor updates its
  saved baseline, reports the warning, and cancels an automatic follow-up action.
  Existing symbolic links and security metadata are preserved. Atomic replacement can
  change inode identity; hard links are not preserved as shared writable aliases.
- Editable file loading and saving run on worker threads. The UI shows progress
  and prevents editing or closing during an operation. Save-before-close/open/new
  proceeds only after successful completion. Save As commits the path on success.
- Normal saves compare disk contents with the opened/saved version immediately
  before replacement. External edits or deletion stop the save. This is conflict
  detection, not a filesystem compare-and-swap: another process can still write
  between the final check and replacement. Save As follows the native dialog's
  explicit replacement confirmation.
- Opening RTF produces an unsaved document and never writes a sibling text file.
  Its sibling `.txt` path is retained only as the suggested Save As location.
- Counts, dirty state and search results are cached until document/query changes.
  Search results are shared between frames instead of copying match vectors.
  Spelling only allocates word spans and performs dictionary lookups around the
  viewport; locating its character offsets still scans the preceding text.
- Unsaved recovery copies are staged atomically by a sequential background worker
  after up to approximately two seconds of changes. Copies survive abnormal
  exits and are offered at startup using a short text preview. Restoration creates
  an unsaved document; it never overwrites the original. Save/discard removes the
  current session copy and the restored source copy. Closing the recovery window
  leaves copies on disk. An abrupt crash can lose edits since the last completed
  snapshot. Copies from another running instance may also be listed.
- Dictionary and update requests have generation IDs. Old successes and failures
  are ignored, and completed jobs wake the UI.

## Reproduce the layout measurements

```sh
cargo test --offline
cargo test --offline --release editor_performance -- --ignored --nocapture
```

The ignored performance test uses a headless egui context, a 900 × 600 viewport,
monospace text and TextEdit inside a vertical ScrollArea. It inserts a character
near the middle of the buffer before each edit frame, then measures unchanged
text while scrolling. First-load timings are excluded. This measures CPU work
for the widget/layout, not native keyboard-to-display latency or GPU rendering.
The dictionary download integration test also remains ignored by default.

Example release results on the development Mac:

| Bytes | Shape | Edit median | Edit max | Scroll median | Scroll max |
| --- | --- | ---: | ---: | ---: | ---: |
| 204,800 | Short lines | 5.4 ms | 10.8 ms | <0.1 ms | 1.6 ms |
| 204,800 | One long line | 9.1 ms | 11.1 ms | <0.1 ms | 0.4 ms |
| 1,048,576 | Short lines | 25.9 ms | 47.5 ms | 0.2 ms | 6.2 ms |
| 1,048,576 | One long line | 50.0 ms | 57.6 ms | 0.1 ms | 1.7 ms |
| 2,097,152 | Short lines | 52.3 ms | 75.5 ms | 0.4 ms | 13.1 ms |
| 2,097,152 | One long line | 103.0 ms | 124.8 ms | 0.2 ms | 3.5 ms |

These are observations, not timing assertions or before/after speedup claims.
Egui already caches layout per paragraph, but a huge single paragraph still
requires a full re-layout. Keeping typing consistently below 16 ms at 1–2 MB
would require a separate editor layout/virtualization change with coverage for
selection, wrapping, IME, undo and search navigation. The existing read-only
viewer above 2 MiB still reads bounded windows synchronously when scrolling;
network-drive latency there is not addressed by the editable-file worker.

Regression tests cover replacement failures, symlinks and permissions, external
changes/deletion, save continuation ordering, dirty-state undo, RTF collisions,
stale dictionary results, recovery persistence/restoration and ordered cleanup.
Native dialogs, recovery window appearance and Windows filesystem behavior still
need manual/platform validation.

## Atomic replacement metadata

On macOS, `fcopyfile(COPYFILE_METADATA)` copies POSIX metadata, ACLs and extended
attributes before writing the temporary file. Ownership and mode are checked
explicitly before replacement. The regression suite verifies ACLs, a custom
extended attribute and ownership on macOS, in addition to the symlink test.

On other Unix platforms, GNU `cp --attributes-only --preserve=mode,ownership,xattr`
is required. Explicit preservation failures stop the save before replacement;
there is no fallback that drops metadata. On Windows, the Win32 security APIs copy the
original owner/group/DACL and `ReplaceFileW` preserves streams and other native
metadata with ACL/merge-error ignoring disabled. These Linux/Windows paths still
need native platform validation.

Platform references: macOS SDK `copyfile.h` definitions and
[Microsoft ReplaceFileW documentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).

Review regression checks: 50 tests passed on macOS, 2 ignored. The tests inject
a post-replacement directory sync failure and verify the resulting save baseline,
subsequent save, and cancellation of automatic follow-up actions. RTF tests verify
both the absence of automatic writes and preservation of the suggested location.

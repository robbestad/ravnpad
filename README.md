<p align="center">
  <img src="https://raw.githubusercontent.com/robbestad/ravnpad/main/ravn-logo.png" alt="RavnPad" width="160">
</p>

<h1 align="center">RavnPad</h1>

<p align="center">
  A fast, honest notepad. Open a file, write, save. That is the whole product.
</p>

<p align="center">
  <a href="https://github.com/robbestad/ravnpad/releases"><img src="https://img.shields.io/github/v/release/robbestad/ravnpad?label=download" alt="GitHub release"></a>
  <a href="https://crates.io/crates/ravnpad"><img src="https://img.shields.io/crates/v/ravnpad.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT"></a>
</p>

Word is for documents. The web is for everything else. RavnPad is for the file in front of you: a log, a note, a dump, a `.txt` you just want to read and change.

It starts instantly, stays out of the way, and does not try to become an IDE.

## Why people keep it

- **Native editing** — AppKit/NSTextView on macOS and Win32/Rich Edit on Windows, with system menus, selection, undo, scrolling, font selection, and Open/Save dialogs.
- **Fifteen languages** — Bokmål, Nynorsk, English, Swedish, Danish, Icelandic, German, Dutch, French, Spanish, Italian, Portuguese, Finnish, Polish, and Czech. Follows the system language when it can.
- **Your font** — pick a system typeface and size. Settings are remembered.
- **Light or dark** — follow the operating-system theme automatically, or choose a theme yourself.
- **Pick up where you left off** — recent files are one click away and reopen at the remembered reading position.
- **Your layout** — toggle wrapping for long lines; the choice also applies to huge-file viewing.
- **RTF in, text out** — open `.rtf` as an unsaved UTF-8 document, with a sibling `.txt` suggested when saving.
- **Huge files** — over 2 MB opens as a read-only windowed view. Scroll a 2 GB log without freezing.
- **Updates itself** — on Windows and macOS, **Help → Check for updates** (release builds also check at startup).
- **Open with** — on Windows, register for `.txt` and friends without admin rights. RavnPad does not steal Word as the default handler.

## Get it

**Windows and macOS:** grab a binary from [GitHub Releases](https://github.com/robbestad/ravnpad/releases).

- Windows: unzip and run `ravnpad.exe`
- Apple Silicon: `ravnpad-macos-aarch64.zip`
- Intel Mac: `ravnpad-macos-x86_64.zip`

Or install from source:

```bash
cargo install ravnpad
```

Then run `ravnpad`, or `ravnpad notes.txt`. Drag a file onto the window (Windows, and Linux on X11).

### macOS first launch

The app is **ad hoc signed**, not Apple-notarized. Gatekeeper may block the first open. If you trust the download from this repository, use **System Settings → Privacy & Security → Open Anyway**. See [Apple’s instructions](https://support.apple.com/en-gb/102445).

If macOS says the app is damaged, check the signature before making an exception:

```bash
codesign --verify --deep --strict --verbose=4 "/Applications/RavnPad.app"
```

If verification fails, download a fresh copy and report it. Do not re-sign the file to hide a failed check.

If verification succeeds, you downloaded it from GitHub Releases, and Open Anyway is unavailable, you can clear the quarantine **for this app only**:

```bash
xattr -dr com.apple.quarantine "/Applications/RavnPad.app"
```

That does not notarize RavnPad. Do not turn Gatekeeper off.

## Shortcuts

| Action     | Shortcut     |
| ---------- | ------------ |
| New        | Ctrl+N       |
| New window | Ctrl+Shift+N |
| Open       | Ctrl+O       |
| Save       | Ctrl+S       |
| Save as    | Ctrl+Shift+S |
| Quit       | Ctrl+Q       |

Unsaved changes ask before New, Open, Quit, and drop. Only UTF-8 is supported.

**macOS:** use ⌘ instead of Ctrl. Settings are in the application menu (⌘,); Find is ⌘F. The native font panel and spelling services are available. The app follows the system appearance.

**Windows:** Settings contains language, font and spelling options. Ctrl+F opens the native Find dialog; Ctrl+H opens Replace. Native text services provide input-method and proofing support where installed.

Native macOS and Windows builds edit files up to 16 MiB. Linux retains the measured 2 MiB edit limit for the egui interface. Larger files are read-only; the file-position slider loads a bounded window in the background, while native Find scans the complete file.

Theme, recent-file, reading-position and line-wrapping preferences are remembered. File dialogs and alerts use the operating system.

## Build

Rust **1.88+**, plus Xcode Command Line Tools on macOS or the MSVC C compiler/Windows SDK on Windows. On Linux you also want `libxcb`, `libxkbcommon`, and GTK 3 (native dialogs).

```bash
cargo run --release
```

Windows:

```bat
cargo build --release
```

The exe is `target\release\ravnpad.exe`. Release builds hide the console.

## Agent access (opt-in)

Start RavnPad with `--enable-agent` to expose the current live document to local
clients for this process only. The endpoint uses an owner-only named pipe on
Windows (remote clients are rejected) or a private Unix socket. Closing RavnPad,
opening another document, or restarting invalidates the document reference.

```bash
ravnpad --enable-agent notes.txt
ravnpad-cli document status --instance INSTANCE_ID --json
ravnpad-cli document read --instance INSTANCE_ID --document DOCUMENT_ID --json
ravnpad-cli document propose --instance INSTANCE_ID --document DOCUMENT_ID --stdin --json < patch.json
```

Instance IDs are the names of the small endpoint files in RavnPad's `agent`
configuration directory. `document status` resolves the current document ID.
Direct file access is deliberately separate and read-only:

```bash
ravnpad-cli file read notes.txt --json
```

Patches use half-open UTF-8 byte ranges and must include `operation_id`,
`document_id`, `base_revision`, `base_hash`, and an `edits` array. Every edit
contains `start_byte`, `end_byte`, `expected_text`, and `replacement`. RavnPad
validates the complete patch before showing a side-by-side proposal. Rejection
changes nothing; approval updates the buffer as one undoable action and does not
save the target file. Stale revisions, invalid UTF-8 boundaries, overlapping
edits, ambiguous insertions, and mixed line endings are rejected.

`ravnpad-mcp` exposes the same status, read, and propose operations as MCP tools
over stdio. It writes only newline-delimited JSON-RPC messages to stdout; logs
and diagnostics go to stderr.

## License and mark

MIT. The Ravn logo is a registered trademark.

## Native control smoke tests

```sh
cargo test --locked
cargo build --release --locked
./target/release/ravnpad --native-smoke-test
```

The smoke test instantiates the actual platform text control and verifies Unicode
round trips, undo/redo, document replacement and read-only loading. It does not
open a user document or load preferences. CI runs it on Windows and both Mac
architectures before producing packages.

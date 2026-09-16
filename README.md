<p align="center">
  <img src="https://raw.githubusercontent.com/robbestad/ravnpad/main/ravn-logo.png" alt="RavnPad" width="160">
</p>

<h1 align="center">RavnPad</h1>

<p align="center">
  A fast notepad. Agents that ask first.
</p>

<p align="center">
  <a href="https://ravnpad.com"><img src="https://img.shields.io/badge/web-ravnpad.com-1e4f9e" alt="ravnpad.com"></a>
  <a href="https://github.com/robbestad/ravnpad/releases"><img src="https://img.shields.io/github/v/release/robbestad/ravnpad?label=download" alt="GitHub release"></a>
  <a href="https://crates.io/crates/ravnpad"><img src="https://img.shields.io/crates/v/ravnpad.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT"></a>
</p>

Open a file. Write. Save.

That is still the whole product.

Need an agent? Start with `--enable-agent`.
It reads the live buffer and applies a validated edit.
It does not save.

Word is for documents. The web is for everything else. RavnPad is for the file in front of you: a log, a note, a dump, a `.txt`. It starts instantly, stays out of the way, and does not try to become an IDE.

## Uncomplicated on purpose

- **Open, write, save** — native AppKit on macOS, Win32 Rich Edit on Windows. System menus, undo, fonts, Find, Open/Save. No project, no sidebar, no cloud account.
- **Fifteen languages** — follows the system language when it can. Bokmål, Nynorsk, English, Swedish, Danish, Icelandic, German, Dutch, French, Spanish, Italian, Portuguese, Finnish, Polish, and Czech.
- **Your font and theme** — pick a system typeface and size. Light, dark, or follow the OS. Choices are remembered, including recents, reading position, and wrap.
- **Huge files** — over 2 MB opens as a read-only windowed view. Scroll a 2 GB log without freezing.
- **RTF in, text out** — open `.rtf` as an unsaved UTF-8 document. A sibling `.txt` is suggested when you save.
- **Updates itself** — Help → Check for updates. Release builds also check at startup on Windows and macOS.
- **Open with** — on Windows, register for `.txt` and friends without admin rights. RavnPad does not steal Word.

## Agents that ask first

Most AI editors become a chat with a text area attached. RavnPad does the opposite: the notepad stays a notepad. Turn on agent access for the current process from **Agent → Enable agent mode**, or start RavnPad with `--enable-agent`.

- **Live buffer, not the disk.** The agent reads what you are looking at, including unsaved edits. It never falls back to the file on disk. Valid edits are applied directly; stale or ambiguous edits are rejected so the agent can reread and merge against the current text.
- **Validated direct edits.** Every patch is bound to the document revision, buffer hash, and expected text. A valid patch becomes one undoable edit and still does not save; a stale or ambiguous patch is rejected for the agent to reread and merge.
- **Local, owner-only.** Windows uses an owner-only named pipe (remote clients are rejected). macOS and Linux use a private Unix socket. No cloud sidecar, no always-on daemon.

```bash
ravnpad --enable-agent notes.txt
ravnpad-cli document status --instance INSTANCE_ID --json
ravnpad-cli document read --instance INSTANCE_ID --document DOCUMENT_ID --json
ravnpad-cli document propose --instance INSTANCE_ID --document DOCUMENT_ID --stdin --json < patch.json
```

Direct file access is separate and read-only:

```bash
ravnpad-cli file read notes.txt --json
```

MCP for Claude, Cursor, Codex, and friends:

```json
{
  "mcpServers": {
    "ravnpad": {
      "command": "ravnpad-mcp"
    }
  }
}
```

Windows zip: `ravnpad-cli.exe` and `ravnpad-mcp.exe` sit beside `ravnpad.exe`. macOS: both tools live in `RavnPad.app/Contents/Helpers/`. Instance IDs are the JSON filenames in RavnPad’s agent config directory.

Paste-ready agent rules: [ravnpad.com](https://ravnpad.com/#agents).

A patch must include `operation_id`, `document_id`, `base_revision`, `base_hash`, and `edits[]` with `start_byte`, `end_byte`, `expected_text`, and `replacement` (half-open UTF-8 byte ranges). Overlaps, invalid UTF-8 boundaries, mixed line endings, NUL, more than 128 edits, or more than 256 KiB of changed text are rejected. Closing RavnPad, opening another document, or restarting drops the handle.

## Get it

Download from [ravnpad.com](https://ravnpad.com) or [GitHub Releases](https://github.com/robbestad/ravnpad/releases).

- Windows: unzip and run `ravnpad.exe`
- Apple Silicon: `ravnpad-macos-aarch64.zip`
- Intel Mac: `ravnpad-macos-x86_64.zip`

macOS builds are Developer ID signed, Apple-notarized, and stapled. Drop `RavnPad.app` into Applications and open it.

Or install from source:

```bash
cargo install ravnpad
```

Then `ravnpad`, or `ravnpad notes.txt`. Drag a file onto the window (Windows, and Linux on X11).

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

**macOS:** use ⌘ instead of Ctrl. Settings are in the application menu (⌘,); Find is ⌘F. Native font panel and spelling. Follows the system appearance.

**Windows:** Settings has language, font, and spelling. Ctrl+F is Find; Ctrl+H is Replace.

Native macOS and Windows builds edit files up to 16 MiB. Linux keeps the measured 2 MiB edit limit for the egui interface. Larger files are read-only.

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

```sh
cargo test --locked
cargo build --release --locked
./target/release/ravnpad --native-smoke-test
```

The smoke test instantiates the actual platform text control and verifies Unicode round trips, undo/redo, document replacement, and read-only loading. It does not open a user document or load preferences. CI runs it on Windows and both Mac architectures before producing packages.

## License and mark

MIT. The Ravn logo is a registered trademark.

# RavnPad

A simple UTF-8 notepad. Open, edit, and save text. Nothing more.

## Install

Prebuilt Windows and macOS binaries are on the [GitHub Releases](https://github.com/robbestad/ravnpad/releases) page.

```bash
cargo install ravnpad
```

### macOS

Download the ZIP for your Mac: `aarch64` for Apple Silicon, or `x86_64` for Intel.
Extract it and move `RavnPad.app` to Applications.

Finder and Dock use the RavnPress logo from `RavnPad.app/Contents/Resources`.
The window icon is the same logo, loaded at runtime. Ad hoc signing does not
remove the icon.

The app uses a free **ad hoc signature**. This checks bundle integrity, but does
not identify the developer to Apple and is **not notarization**. No paid Apple
account is needed to build these releases. Gatekeeper may still block an app
downloaded from the internet.

After trying to open it, use **System Settings → Privacy & Security → Open Anyway**
if available, and only if you trust the download. See
[Apple's instructions](https://support.apple.com/en-gb/102445).

If macOS says the app is damaged, check its signature before making an exception:

```bash
codesign --verify --deep --strict --verbose=4 "/Applications/RavnPad.app"
```

If verification fails, download a fresh copy of the latest release and report
the error if it persists. Do not re-sign the downloaded copy to hide the failure.

If verification succeeds, you downloaded it directly from this repository's
GitHub Releases, and manual approval is unavailable, you can explicitly remove
the download quarantine **for this app only**:

```bash
xattr -dr com.apple.quarantine "/Applications/RavnPad.app"
```

This bypasses the download quarantine check for RavnPad; it does not notarize it
or prove the download is trustworthy. Do not disable Gatekeeper system-wide.

Release builds verify the app signature both before packaging and after extracting
the ZIP. This does not replace testing the downloaded app on a Mac with Gatekeeper
enabled.

## Requirements

- [Rust](https://rustup.rs/) 1.88 or newer
- On Linux: common GUI libraries (`libxcb`, `libxkbcommon`, GTK 3 for file dialogs)

## Run

```bash
cargo run --release
```

## Build a Windows exe

On a Windows machine with Rust installed:

```bat
cargo build --release
```

The binary is `target\release\ravnpad.exe`. Release builds hide the console window. The app icon is the RavnPress logo.

On Windows, RavnPad registers itself for text files (`.txt`, `.text`, `.log`, `.md`) at startup, without administrator rights. Double-clicking those files opens them in RavnPad. If Windows already has another default app, choose RavnPad under **Open with**.

## Usage

| Action    | Shortcut     |
| --------- | ------------ |
| New       | Ctrl+N       |
| Open      | Ctrl+O       |
| Save      | Ctrl+S       |
| Save as   | Ctrl+Shift+S |
| Quit      | Ctrl+Q       |

**Settings** has language, editor font, and font size. RavnPad starts in the system language when it is one of the built-in ones (Norwegian Bokmål and Nynorsk, English, Swedish, Danish, Icelandic, German, Dutch, French, Spanish, Italian, Portuguese, Finnish, Polish, and Czech). Settings are remembered.

On Windows and macOS, RavnPad checks [GitHub Releases](https://github.com/robbestad/ravnpad/releases) for a newer version at startup. Use **Help → Check for updates** to check now; if an update is found, RavnPad downloads it and restarts. Linux builds from `cargo install` are not auto-updated.

Unsaved changes prompt before New, Open, Quit, and drag-and-drop. Only UTF-8 is supported. Opening an `.rtf` file converts it to plain text and saves a `.txt` file next to it. Files larger than 2 MB open as a read-only view (scroll through the file without loading it all into memory), so large logs do not freeze the app.

Open a file from the command line with `ravnpad file.txt` (or drop the file on the program icon). Drag-and-drop into the window works on Windows and on Linux via X11.

The Ravn logo is a registered trademark.

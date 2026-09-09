# RavnPad

A simple UTF-8 notepad. Open, edit, and save text. Nothing more.

## Install

```bash
cargo install ravnpad
```

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

Unsaved changes prompt before New, Open, Quit, and drag-and-drop. Only UTF-8 is supported. Files larger than 2 MB open as a read-only view (scroll through the file without loading it all into memory), so large logs do not freeze the app.

Open a file from the command line with `ravnpad file.txt` (or drop the file on the program icon). Drag-and-drop into the window works on Windows and on Linux via X11.

The Ravn logo is a registered trademark.

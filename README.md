# RavnPad

Enkel les/skriv-notisblokk. Åpne, rediger og lagre UTF-8-tekst. Ingenting mer.

## Krav

- [Rust](https://rustup.rs/) (1.85 eller nyere)
- På Linux: vanlige GUI-biblioteker (`libxcb`, `libxkbcommon`, GTK 3 for fil-dialoger)

## Kjør

```bash
cargo run --release
```

## Bygg Windows-exe

På en Windows-maskin med Rust installert:

```bat
cargo build --release
```

Resultatet ligger i `target\release\ravnpad.exe`. Release-bygg skjuler konsollvinduet. Programikonet er RavnPress-logoen.

På Windows registrerer RavnPad seg som program for tekstfiler (`.txt`, `.text`, `.log`, `.md`) ved oppstart, uten administrator. Dobbeltklikk åpner filen i RavnPad. Hvis Windows allerede har en annen standardapp, velg RavnPad under **Åpne med**.

## Bruk

| Handling     | Snarvei         |
| ------------ | --------------- |
| Ny           | Ctrl+N          |
| Åpne         | Ctrl+O          |
| Lagre        | Ctrl+S          |
| Lagre som    | Ctrl+Shift+S    |
| Avslutt      | Ctrl+Q          |

Ulagrede endringer spør før Ny, Åpne, Avslutt og dra-og-slipp. Bare UTF-8 støttes.

Åpne en fil fra kommandolinjen med `ravnpad fil.txt` (eller dra filen på programikonet). Dra-og-slipp inn i vinduet virker på Windows og på Linux via X11.

Ravn-logoen er et registrert varemerke.

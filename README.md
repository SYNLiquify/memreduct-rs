# Mem Reduct (Rust)

A lightweight, real-time memory monitor and cleaner for Windows, rewritten from scratch in Rust.

**Rust rewrite by Liquify.**
Based on [Mem Reduct](https://github.com/henrypp/memreduct) by **Henry++**, the original author. All credit for the original application, its design and its memory-cleaning approach goes to Henry++.

Release numbers follow the upstream project: `v.3.5.2` of this rewrite corresponds to Mem Reduct 3.5.2.

## Features

- **Live monitoring**: physical memory, pagefile and system working set usage, shown in the main window and as a percentage icon in the notification area.
- **Memory cleaning** through the Windows Native API (`NtSetSystemInformation`), the same calls as the original:
  - Working set
  - System file cache
  - Modified file cache (flushes every volume)
  - Standby list (without priority)
  - Standby list\*
  - Modified page list\*
  - Registry cache (Windows 8.1+)
  - Combine memory lists (Windows 10+)

  \* These can make the computer unresponsive for a while, so they are off by default and excluded from auto-clean unless you allow them.
- **Automatic cleaning** when usage goes above a threshold, or every N minutes.
- **Global hotkey** (default `Ctrl+F1`).
- **Tray icon styling**: font, colors, transparency, border, round corners, and warning/danger levels.
- **Interface themes** (new in the rewrite): System default, Light, Dark, Midnight, Forest, Sunset, or Custom with your own colors for background, text, accent, accent text, lists/inputs and borders. Switch from *View → Theme* or *Settings → Theme*.
- Load on startup, start minimized, always on top, and skipping the UAC prompt through a scheduled task.
- Portable mode: put a `memreduct.ini` next to `memreduct.exe`.

## Download

Get `memreduct.exe` from the [releases page](https://github.com/SYNLiquify/memreduct-rs/releases). It's a single file with nothing to install. Cleaning memory needs administrator rights; the app asks for them through UAC when needed.

Requires 64-bit Windows 10 (version 1607 or later) or Windows 11.

## Command line

```
memreduct.exe -clean          clean the configured memory regions, then exit
memreduct.exe -clean:full     clean all memory regions, then exit
memreduct.exe -clean -silent  same, without the result message box
memreduct.exe -minimized      start hidden in the notification area
memreduct.exe -help           list the options
```

## Settings

Settings live in `%APPDATA%\memreduct-rs\memreduct.ini`. If a `memreduct.ini` exists next to the executable, that file is used instead (portable mode). Key names match the original Mem Reduct, so an existing `[memreduct]` section can be copied over.

## Differences from the original

- Written in Rust, with the Win32 API used directly through `windows-sys`. All dialogs are built in code, and the icon and version resources are produced by `build.rs`, so no resource compiler is needed.
- New interface theming (see above) and a recolored app icon.
- Memory cleaning runs on a background thread, so the window stays responsive while standby lists are purged.
- `-silent` option for scripted cleaning.
- English only. No built-in updater: check the releases page for new versions.

## Building

Requires Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools.

```
cargo build --release
```

The executable is written to `target\release\memreduct.exe`. Run the unit tests with `cargo test`.

## License

GNU General Public License v3.0 or later, the same license as the original Mem Reduct. See [LICENSE](LICENSE).

- Mem Reduct (original): Copyright (c) 2011-2026 Henry++. <https://github.com/henrypp/memreduct>
- Rust rewrite: Copyright (c) 2026 Liquify. <https://github.com/SYNLiquify/memreduct-rs>

The application icon is derived from the original Mem Reduct icon (recolored).

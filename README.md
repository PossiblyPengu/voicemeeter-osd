# Voicemeeter OSD

An on-screen volume display for [Voicemeeter](https://vb-audio.com/Voicemeeter/). Voicemeeter has no
equivalent of the Windows volume popup, so when a hardware knob or a MIDI control moves a fader there
is no visual feedback unless the Voicemeeter window is in front of you.

This is a small tray app that watches one strip or bus and shows a bar whenever its level changes.

<p align="center">
  <img src="assets/screenshot-settings.png" width="420" alt="Settings window">
</p>

## Features

- **Pops out of the tray icon.** The bar grows out of the notification-area icon and retracts into it,
  with a callout tail pointing back at the icon.
- **Vertical or horizontal**, docked above the tray or floating at the bottom centre of the screen.
- **Reads the level, the dB value and the channel name** — using the label you typed in Voicemeeter
  if you set one, otherwise `A1`, `B2`, `Strip 3` and so on.
- **Scroll the tray icon** to change the volume.
- **Click the bar** to bring up the Voicemeeter window.
- **Multi-monitor aware**, including per-monitor DPI. Docked mode follows the display that owns the
  tray; floating mode follows the display you are working on.
- **Stays out of the way** of fullscreen games and presentations.
- Optional **Windows accent colour** matching, adjustable opacity, timeout and fader range.
- Runs at startup if you want it to, and installs itself somewhere stable when you enable that.

## Requirements

- Windows 10 or 11
- Voicemeeter, Voicemeeter Banana, or Voicemeeter Potato

## Install

Download `voicemeeter-osd.exe` from the [latest release](../../releases/latest) and run it. There is
no installer and nothing to unpack — it is a single binary with no runtime dependencies beyond the
system DLLs that ship with Windows.

It appears in the notification area. On Windows 11 new tray icons start hidden behind the `^` chevron;
drag it onto the taskbar to keep it visible.

To have it start with Windows, turn on **Start with Windows** in settings. That copies the binary to
`%LOCALAPPDATA%\Programs\VoicemeeterOSD\` and points the startup entry there, so it keeps working if
you move or delete the copy you downloaded.

## Setup

Right-click the tray icon and choose **Settings**.

The one thing you need to get right is **which channel to watch**. Open Voicemeeter, move your knob or
fader, and note which column responds — then set the matching type and index:

- **Bus** covers the outputs: index 0 is `A1`, 1 is `A2`, and the `B` buses follow the physical ones.
- **Strip** covers the inputs, numbered left to right from 0.

If the percentage doesn't line up with where the fader looks, adjust the **fader range** — Voicemeeter
faders run from -60 dB to +12 dB by default, and the percentage is mapped across that range.

Settings are stored in `%APPDATA%\voicemeeter-osd\settings.txt`.

## Building

Requires a Rust toolchain with the MSVC target, and the Windows SDK for `rc.exe` if you want the icon
embedded in the executable (it builds fine without, just without a file icon).

```sh
cargo build --release
```

The icon is generated rather than hand-drawn; `tools/make_icon.ps1` redraws it at every size and packs
the `.ico`. `tools/watch_params.ps1` logs every strip and bus gain, which is a quick way to find out
which parameter a hardware control actually drives.

## How it works

It talks to Voicemeeter through `VoicemeeterRemote64.dll`, the Remote API that ships with Voicemeeter,
polling the watched channel and showing the bar when the value changes.

The bar is a layered window drawn with GDI+ into a premultiplied-ARGB surface and pushed through
`UpdateLayeredWindow`, which is what gives it antialiased corners, a soft shadow and per-pixel
transparency. There is no web view and no UI framework — the whole thing is Win32 plus one crate
(`windows-sys`), which keeps the binary around 260 KB.

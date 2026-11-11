# Prisma

**English** · [Português (Brasil)](README.pt-BR.md) · [Español](README.es.md) · [Русский](README.ru.md) · [简体中文](README.zh-CN.md)

A lightweight utility for Windows 11, written entirely in Rust with no UI framework. It sits in the tray, loads only what is in use and fits in a single 2 MB file. It replaces Task Manager and brings together the small tools a PC gamer usually collects one by one.

**[Download the latest version](https://github.com/maliquem/Prisma/releases/latest)** · **[Guide: every feature explained](docs/guide/GUIDE.md)**

[![Buy Me a Coffee at ko-fi.com](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/maliquem)

![Prisma widget styles](docs/widget.png)

## Getting started

Prisma is one file, `Prisma.exe`. Windows may warn about an unknown publisher the first time, because the file is not code-signed.

On the first open, the welcome screen asks if you want to install, give administrator permission and replace Task Manager. All three come checked, and Windows asks for permission once. The screen also opens the guide, which explains each feature and what it can and can't do. It appears only once, unless you uncheck **Don't show again**.

- **Portable:** run it from any folder. Settings stay in `PrismaData`, next to the program, and nothing is registered in Windows. With administrator, Windows asks every time it opens.
- **Installed:** Prisma moves to Program Files, gets a Start menu shortcut and appears in the Windows program list. With administrator, it opens without asking. You can also install later, in **Settings**.

The installed version starts with Windows by default, and you can turn that off in **Settings**. The portable version does not start with Windows. Task Manager can be replaced in both, but on the portable version the shortcut stops working if you move or delete the file.

The guide and **Report a bug** are two buttons next to the Prisma name, at the top of the window. Hover over them to see what each one does.

The interface comes in English, Brazilian Portuguese, Spanish, Russian and Simplified Chinese. Prisma starts in your Windows language, and you can change it in **Settings**. The guide and the notes for each version come in English and Portuguese; the other languages show them in English. Text that comes from Windows itself, such as service descriptions and command output, stays in the Windows language.

## Updates

Prisma checks this page once a day. When there is a new version and you open the window, it asks if you want to update. Minimized or in the tray, it does not interrupt you. If you answer Yes, it downloads the file from this page in parts, checks its SHA-256 hash, swaps the program and restarts. The check sends no data of yours. Check **Don't show again** in the question, or turn it off in **Settings**, to stop it. In the installed version, Windows asks for permission if Prisma is not running as administrator.

## Features

- **Monitor:** per-core CPU, GPU, RAM, VRAM, disk and network for each program, with icons. Right-click a program to end the task, go to details, open the file location, search online or see its properties. The Details tab lists every process with its PID. Search by name or PID, and restart Explorer when the taskbar freezes.
- **Disks and networks:** speed, active time and free space for each disk, plus a health grade (Good, Warning, At risk) from SMART, including USB drives. Each network shows speed, address and total transferred.
- **Disk space:** analyzes a whole disk by reading the NTFS table, with folders, largest files, file types and a visual map. Opens in Explorer and, as administrator, sends files to the Recycle Bin.
- **Search:** finds files and folders by name on all disks as you type, from the tab or from a pop-up on Ctrl + Alt + Space. As administrator, the index comes from the NTFS table in seconds and follows every change live. Installed programs come first, questions go to the browser or an installed AI, and `>` runs PowerShell or CMD commands with suggestions. Optional, off by default.
- **Startup:** turns on, turns off, adds and removes what opens with Windows, and shows who signed each program. Sorts by name, enabled or disabled.
- **Services:** description, start, stop and restart. Right-click a service for start, stop, restart and properties, which opens its own Windows properties window.
- **Uninstall:** removes in batches, creates a restore point and looks for leftovers, then tells you what was deleted. Sorts by name, date or size. Only exact name matches come selected, similar names appear for you to review, folders go to the Recycle Bin and the registry gets a backup first.
- **Force close:** a configurable shortcut, Alt+F4 by default, ends the focused program by PID at once, even when it is frozen in fullscreen. Unsaved work is lost. The desktop, the taskbar and Windows processes keep the normal Alt+F4 behavior.
- **Replace Task Manager:** a button in Settings makes Ctrl+Shift+Esc open Prisma, and another gives it back. Uninstalling also gives Task Manager back.
- **Windows tweaks:** first a read-only audit, then layered Gaming, Development and Privacy profiles with documented effects only. The Left out tab explains what was excluded and why. Each change records the previous value, and Undo only restores what nobody touched afterwards. No profile touches updates, antivirus, UAC, Memory Integrity or virtualization. **Show log** displays every change, before and after.
- **Tools:** Windows repair (DISM, then SFC), crash history, Windows Update reset, disk check, network, DNS, cleanup, winget, diagnostics and advanced restart. On a suspect disk, modes that write are blocked. Each tool keeps its own step-by-step log, with the exact command, its output, errors and the time of each step.
- **Optimize:** suggests what to close and always asks. The never-close list is respected, the always-close list closes those programs, and automatic detection suggests names for both. The question when a game opens comes off and can be turned on in Power.
- **Meter:** compact bar and full panel over the game, only through the shortcut, which measures the program in focus, adds the game to the list and hides the meter when pressed again. Shows FPS, 1% and 0.1% lows, frame time, CPU, GPU, VRAM and RAM. The compact bar can also show RAM, CPU and GPU temperature and the power plan, plus the limiter mode, lit while the limit is acting. CPU temperature needs PawnIO, a free open driver that Prisma can install from the Meter page. The readings come from Windows events, with nothing inside the game. FPS limit per game, manual or automatic after 5 minutes of play. By default a small Prisma module enters the game when the limiter is on, so the limit changes right away in DirectX 10, 11 and 12. If it fails, the game crashes or an anti-cheat is detected, the limit moves to the NVIDIA driver and applies the next time the game opens. The buttons are in the full panel: Ctrl + Alt + 5 unlocks mouse and keyboard for them, and you can type the limit. It also picks the DLSS profile (only in games with DLSS), the Windows Auto HDR per game, the same as in the HDR tab, and the power plan (also a plan per game that turns on when it opens; unused games can leave the list after some days, keeping their settings), shows the loaded upscaler, frame generation and Reflex, and turns on Windows HDR for listed games or all of them, without turning off HDR that was already on. Needs Prisma running as administrator. In exclusive fullscreen the bar may not appear.
- **Data log:** an optional recording for studying performance, started from the Meter page or with a shortcut during the game. It saves every frame, every second and the CPU of each game thread in `Documents\Prisma\Logs`, with a summary at the end; on NVIDIA cards, also the GPU (memory controller, power and its limit, what held the clock back, PCIe and VRAM), for studying optimization mods. Type the minutes of game time (5 is the default, 0 is no limit). It runs only if you turn it on, and thread reading stays off in games with a detected anti-cheat. While it records, the meter shows a red Recording warning, in the compact bar and in the full panel. The summary also says the limit in force and the frame generation: which one the game loaded, how long it was on and by how many times.
- **Power:** detects a game or high load and switches the power plan, going back to rest after 60 s without load. With automatic switching off, a fixed plan stays on until it comes back. You choose the percentage and time that count as load for CPU and GPU. Select a plan to edit it with every option Windows registers, including hidden ones, or to import and export .pow files. Right-clicking a plan also offers Export and Edit. The first edit keeps a copy of the original plan.
- **Per-game profile:** remembers the volume of each program.
- **Fullscreenizer:** turns the focused window into borderless fullscreen. Programs you use it on join a saved list, where you can add them from the program list or by their .exe, remove them and mark Automatic to apply it whenever they open.
- **Sound:** volume per program, and the keyboard volume keys, with a modifier, change only the focused program. Choose the output and input device of each program, including programs added by hand from the program list or by their .exe. The list has what the Start menu shows, Microsoft Store apps, installed programs, Steam and Epic games and whatever is open. This uses an internal Windows feature, and some programs only switch device after they reopen. Pinned programs stay on the list even when closed and come back at their saved volume. Default device shortcuts switch the Windows output and input at once, each with the devices you pick. Keep the sound awake plays a stream with no audible sound, so HDMI and Dolby do not cut the start of the next sound, and it stops by itself after 10 idle minutes.
- **Widget:** CPU, GPU, RAM and VRAM over everything, in four styles.
- **HDR:** a read-only panel with the display mode, the active calibration profile and its real saturation, the brightness each source declares, what Windows tells games, the declared Dolby Vision support and the accepted sound formats. It warns when something does not match. Reading the display's own data directly works only on NVIDIA cards. Your own calibration opens full screen test patterns for black, peak at five window sizes and each color channel, and saves one profile per TV picture mode with the display's real primaries. Windows may keep reporting the old values to games for a while after activation. The profile does not change what the TV does. The Auto HDR list forces it, turns it off or leaves it to Windows per game, for DX11 and DX12 games, with nothing injected into the game. The sound delay test plays a click with a flash, to compare the delay with and without spatial sound.
- **NVIDIA driver:** only with an NVIDIA card. Driver options in the global profile or per game, like NVIDIA Profile Inspector, without the ones that no longer do anything (SLI, 3D Vision, Ansel, CSAA, MFAA, debug and old OpenGL). Sync and FPS, DLSS and frame generation, anti-aliasing and sharpening, textures, RTX HDR and indicators, and system options, each explained on hover, with Default to go back. A backup of every driver profile is saved before the first change.
- **Capture:** photos with annotations and video in AV1, HEVC or H.264. When the display is in HDR, it records in SDR or HDR10. Record Window records the focused window, and Record Screen records the whole screen. With System sound on, it records all PC sounds. With it off, Record Window records only the sound of that window.
- **Replay:** in game mode, keeps the last seconds in memory.
- **Share screen (in testing):** shows and controls another PC over the internet, for remote access or local co-op from a distance, with no account. Pair once with a short code (like `K7M2-QX9P`) or a link that opens Prisma, check a confirmation code, then connect directly, through your VPN or through a relay, always end-to-end encrypted. Permissions per device (sound, controller, keyboard, mouse, copy and paste, secure screen), up to 4 people with virtual Xbox controllers (needs ViGEmBus or VIIPER on the host), HDR converted or turned off while someone watches, and an optional Windows service for the lock screen, UAC and Ctrl+Alt+Del. Settings for resolution, frame rate, codec (including the experimental PyroWave, with HDR10 and 4:4:4, for the local network or a VPN only), bandwidth, priority, sound source, port and shortcuts, plus immersive mode, a latency meter, a warning when the connection gets worse and a connection menu with a shortcut for each option. Host settings apply during the connection, the call app that is playing leaves the sound, and on laptops with two graphics cards you pick which one encodes and which one decodes. Paired devices sit in two tabs (the ones this PC watches and the ones watching it) and can be renamed; changing the permissions of a connected device applies right away. The host gets a Connected tab to disconnect, ban or change access on the spot, and a notification when someone connects or leaves. Ctrl + Alt + Shift + Q on the host disconnects everyone. **Mouse and keyboard** (like Deskflow) uses this PC's mouse and keyboard on up to two paired PCs next to it, with no video: you place each screen on the left, in the middle or on the right, the mouse crosses from one screen to the next, and Scroll Lock locks it to the current screen. **Sound only** (like AudioRelay) plays the paired PC's sound on this one, with no video.
- **Report a bug:** click the bug button at the top of the window. Choose the feature and severity, describe what happened and add a picture if you want. Your browser opens GitHub with everything written and you only confirm. The picture goes to the clipboard: just paste it with Ctrl+V. You can include PC data and a short log, with no user name, PC name or file paths. GitHub pages are public, and you see everything before sending.
- **Crash report:** if Prisma crashes, it saves a report in its data folder and tells you the next time it opens. The file stays on your PC. In **Report a bug**, only a summary of the error can go along, and only if you send it.

![Windows tweaks profiles](docs/profiles.png)

Actions that change something ask for confirmation in a pop-up, every list has a draggable scroll bar, and the Default button on each page puts its options back.

## Default shortcuts

All of them can be changed in the program.

| Action | Shortcut |
| --- | --- |
| Fullscreenizer | Ctrl + Alt + F |
| Focused program volume | Ctrl + volume and mute keys |
| Widget | Ctrl + Alt + W |
| Optimize | Ctrl + Alt + O |
| Area photo | Ctrl + Alt + P (Print Screen works too, if you set it) |
| Show compact meter | Ctrl + Alt + 1 |
| Show full meter | Ctrl + Alt + 2 |
| Hide meter | Ctrl + Alt + 0 |
| Meter controls | Ctrl + Alt + 5 |
| Limiter mode (Automatic, Manual, Off) | Ctrl + Alt + 3 |
| Raise or lower the limit | Ctrl + Alt + ↑ or ↓ |
| Start or stop the data log | Ctrl + Alt + 4 |
| Full screen photo | Ctrl + Alt + A |
| Focused window photo | Ctrl + Alt + J |
| Record Window and stop | Ctrl + Alt + R |
| Record Screen and stop | Ctrl + Alt + T |
| Save replay | Ctrl + Alt + B |

On a photo: Enter or Ctrl + C copies, Ctrl + S saves, Ctrl + Z undoes and Esc cancels. Hover a button to see what it does and its key. Pinned photo: drag to move, right-click closes.

## Limits

- A game in exclusive fullscreen must be in windowed mode before using Fullscreenizer.
- Do not use injection in online games. Anti-cheat detection recognizes known files and titles, and finding nothing does not prove a game has none. You can choose Driver as the method for any game in the Meter page.
- DirectX 9 and Vulkan games use the driver limit, which applies on reopening.
- No CPU temperature or power reading, since they need a kernel driver.
- Profiles do not promise FPS. The value is privacy and less noise.
- Removed apps come back through the Store, and that can fail.
- The health grade is an indicator, not a guarantee: a disk can fail without warning.
- The network column, disk health and fast NTFS reading need Prisma as administrator.
- Everything Prisma changes in the system is recorded in `acoes.log`, in its data folder.
- H.264 has no 10-bit mode: recording HDR10 uses HEVC instead.
- Protected content, such as Netflix, comes out black in captures.
- Share screen is in testing. Without a direct path or VPN, the connection goes through n0's public relays (encrypted end to end). The short code stays for 10 minutes on three public name relays (dns.iroh.link, relay.pkarr.org and pkarr.pubky.org), signed and without anything that identifies you; if none answers, the long invite works without them. It does not send the viewer's microphone.

## Requirements

64-bit Windows 11.

- Recording video needs a video encoder on the GPU (NVIDIA, AMD or Intel) and, on Windows N editions, the Media Feature Pack. Without it, only recording is unavailable.
- GPU temperature depends on the NVIDIA driver. The other readings work on any card.

## Building

```bash
cargo build --release
```

Needs Rust and the Visual Studio Build Tools.

## Third-party software

Share screen uses open-source libraries, unmodified:

- [iroh and iroh-dns](https://github.com/n0-computer/iroh) (MIT or Apache-2.0), which traverses NAT and, without a direct path, relays the connection through n0's public relays, always end-to-end encrypted.
- [attohttpc](https://github.com/sbstp/attohttpc) (MPL-2.0), used by iroh to open router ports via UPnP. The source code is at the link.
- [PyroWave](https://github.com/Themaister/pyrowave) (MIT), an experimental codec for the local network. Its library (with Granite, volk and the Vulkan headers, all MIT) comes inside Prisma and is written to disk with its license only when the codec is used.
- [PawnIO modules](https://github.com/namazso/PawnIO.Modules) (LGPL-2.1), AMDFamily17 and IntelMSR, which run inside the PawnIO driver to read the CPU temperature. Prisma talks to the driver only through its control interface; the driver is installed separately, from its official installer. License and source in `res/pawnio/LEIAME.txt`.
- [tokio](https://github.com/tokio-rs/tokio) (MIT), [blake3](https://github.com/BLAKE3-team/BLAKE3) (CC0-1.0 or Apache-2.0), [data-encoding](https://github.com/ia0/data-encoding) (MIT), [mini-nvcodec and mini-cuda](https://github.com/AdrianEddy/wgpu-interop) (MIT or Apache-2.0), [vigem-client](https://github.com/CasualX/vigem-client) (MIT), [viiper-client](https://github.com/Alia5/VIIPER) (MIT), [url](https://github.com/servo/rust-url) (MIT or Apache-2.0) and [webpki-roots](https://github.com/rustls/webpki-roots) (CDLA-Permissive-2.0, the list of certificate authorities).

The list of settings on the NVIDIA driver page was studied from [NVIDIA Profile Inspector](https://github.com/Orbmu2k/nvidiaProfileInspector) (MIT, Orbmu2k and contributors): setting ids and values only, no code.

ViGEmBus and VIIPER do not come with Prisma: anyone who wants virtual controllers installs one of them themselves.

## License

[PolyForm Noncommercial 1.0.0](LICENSE). You can use, change and share Prisma for any noncommercial purpose. Selling it or using it commercially needs separate permission from the author.

## Publishing a version

1. Update `version` in `Cargo.toml`, add the section to `CHANGELOG.md` and update both READMEs and both guides in `docs/guide`. The program uses those same files.
2. Commit and push a tag such as `v0.3.0`.

GitHub then builds `Prisma.exe` and creates the release as a draft.

3. On the PC that holds the update key, check out the tag and run `tools/publish.ps1 -Version 0.3.0`. It builds again, signs `Prisma.exe` with the key from `tools/update-key.ps1`, replaces the draft files and publishes. Every copy of Prisma then offers the update.

Prisma refuses an update without that signature, so access to the GitHub account alone cannot ship a version. The private key never goes to git or GitHub: it lives in `%LOCALAPPDATA%\PrismaRelease`, encrypted by Windows for that account. Keep a backup made with `tools/update-key.ps1 -Backup`; without the key, installed copies can no longer update on their own.

# Prisma Guide

[Português](GUIA.md)

## Getting started

Prisma is a single file, Prisma.exe. The first time you open it, the welcome screen shows three choices, all checked. Uncheck what you don't want and click **Continue**. Windows asks for administrator permission once and applies everything together.

![Welcome screen](welcome-en.png)

- **Install on this PC:** copies Prisma to Program Files, creates a Start menu shortcut, adds it to the Windows program list and makes it start with Windows. Without this, it runs from its current folder as the portable version, and settings stay in PrismaData, next to the file.
- **Give administrator permission:** the Meter, the network column, disk health and the fast NTFS scan need it. Installed, Prisma opens as administrator without asking. Portable, Windows asks every time, because a file outside Program Files can be replaced by another program.
- **Replace Task Manager:** Ctrl+Shift+Esc opens Prisma. It works only in the installed version: in an ordinary folder, any program could swap the file the shortcut opens, and the shortcut applies to every account on the PC. Without Install on this PC checked, this option is grayed out.
- **Don't show again:** checked by default, so the screen appears once. Unchecked, it appears every time the program opens again. You can change this in Settings.

The screen also has the language selector. Prisma starts in the Windows language: Portuguese, English, Spanish, Russian or Simplified Chinese, following Windows, and English on all others.

All of this can be changed later in **Settings**. Prisma stays in the Windows tray: closing the window does not end the program. To exit, use the menu on the tray icon.

## Guide and Report a bug

Both sit next to the Prisma name, at the top of the window, always in view. Hover over a button to see what it does. In the guide, clicking a topic on the left jumps straight to it.

![Guide and Report a bug buttons](help-en.png)

## Monitor

Shows what is using your PC right now: CPU per core, GPU, RAM and VRAM, plus the CPU, RAM, GPU, VRAM, disk and network use of each program.

- **You can:** end a task, go to details, open the file location, search online and see properties with the right mouse button. The PID column shows each program's number; a program with several processes shows a dash, and the Details tab lists each process with its own PID.
- **Search:** the field next to the tabs filters the list by program or file name. Type only numbers to find a PID.
- **Restart Explorer:** for when the taskbar, the Start menu or the desktop freezes. Prisma asks first, ends Explorer by its PID and it comes back on its own in a few seconds. Open folder windows close.
- **You can't:** see CPU temperature and power, because they require a kernel driver. The network column requires Prisma as administrator. GPU temperature depends on the NVIDIA driver.

## Meter

Shows performance over the game and limits FPS. Readings come from Windows events, with nothing inside the game. It needs Prisma as administrator.

It only shows up through the shortcut, and measures the program in focus when you press it. Turning it on by shortcut in a game adds that game to the list, to keep its limit, HDR and profile; pressing the same shortcut again hides the meter, and the game stays on the list. **Add from the program list** also brings the games installed through Steam and Epic.

- **Automatic HDR:** turns on Windows HDR when the game opens and off when it closes. If HDR was already on, it stays on. **Only for listed games** covers the games on the list; **All** covers every game the Power game mode recognizes, and **Do not turn on**, in the list, leaves a game out.

### Compact bar

Sits at the top of the screen and shows FPS, the frame time line, latency, CPU, GPU and VRAM. Choose left, center or right. Shortcut: Ctrl + Alt + 1.

![Compact meter bar](meter-compact-en.png)

The line shows how frames vary. A straight line means even frames. Variations up to 1 ms are ignored, and large spikes point to stutter.

### Full panel

Sits on the right or left of the screen and shows everything in one column. Shortcut: Ctrl + Alt + 2. In the Meter page you choose what appears and the opacity.

![Full meter panel](meter-full-en.png)

- **FPS and averages:** the big number is the current FPS. 1% low and 0.1% low show the worst moments. The closer they are to the average, the steadier the game.
- **Frame time:** the time of each frame, in milliseconds. Low variation is good.
- **PC latency:** appears only in games with Reflex.
- **Upscaling, frame generation and Reflex:** shows what the game loaded.
- **Power plan:** shows the Windows plan in use, and the arrows switch to another.

To use the panel buttons inside a game, press **Ctrl + Alt + 5**. The game stays in front, and this unlocks the mouse and keyboard: arrows move through the options and change the chosen one, and clicks work too. To write the limit, type the number and press Enter. Press the shortcut again, or Esc, to go back to the game. Ctrl + Alt + 0 hides the meter and resets the readings.

### FPS limiter

With the limiter on, each game keeps its own limit. In the game list on the Meter page you choose the mode, limit, method and what is being limited.

![Options of a game on the Meter page](limiter-en.png)

- **Limiter:** Manual uses the number you choose. Automatic measures 5 minutes of play and picks a limit the game can hold.
- **Injection method:** the default. A small Prisma module enters the game and the limit changes right away, in DirectX 10, 11 and 12.
- **Driver method:** writes the limit to the NVIDIA driver. It applies only the next time the game opens.
- **Limiting:** Real FPS counts the frames the game makes. With generation counts the frames that reach the screen.
- **DLSS profile:** chooses the profile the NVIDIA driver uses for that game. It applies on reopening the game. It only appears for games that ship DLSS; for the others, the column says No DLSS.
- **Auto HDR:** the same Auto HDR per game choice as the HDR tab (Default, Force or Turn off). Changing it here changes it there.
- **Shortcuts:** Ctrl + Alt + 3 turns it on and off. Ctrl + Alt + up or down arrow changes the limit. The limit goes from 20 to 1023.

If injection fails, the game crashes or an anti-cheat is detected, Prisma moves to the driver on its own and tells you.

- **Do not use injection in online games.** Anti-cheat detection recognizes known files and titles, but finding nothing does not prove a game has none. In those games, choose the Driver method.
- **You can't:** use injection in DirectX 9 and Vulkan. Those games use the driver limit, which applies on reopening. The driver limit requires an NVIDIA card. In exclusive fullscreen, the bar may not appear.

### Data log

Records performance to study later. It only runs if you turn it on, on the Meter page or with Ctrl + Alt + 4. Type how many minutes of play to record (5 is the default, 0 is no limit).

Files go to Documents, Prisma, Logs: every frame, every second, the CPU of each game thread and a summary at the end. On NVIDIA cards, each second also carries the GPU for optimization work: memory controller use, power against the limit, what held the clock back (power, temperature), PCIe traffic and total VRAM; the top of the file has the driver, the maximum clocks, the power limit and the PCIe link, and the summary says whether the limit was power, temperature or VRAM. In games with a detected anti-cheat, the thread reading stays off.

## Disks and networks

Shows speed, active time and space of each disk, with a SMART health grade: Good, Warning or At risk, including USB drives. Each network shows speed, address and total transferred.

- **You can't:** treat the grade as a guarantee. A disk can fail without warning. Health requires Prisma as administrator.

## Disk space

Scans a whole disk and shows folders, largest files, types and a visual map.

- **You can:** open items in Explorer and, as administrator, send them to the Recycle Bin.
- **You can't:** use the fast scan without administrator or on a disk that is not NTFS.

## Search

Finds files and folders by name on all your disks, as you type. It comes off: press **Turn on** in the Search tab.

Prisma keeps the names of every file in an index, in memory and on disk, so it answers instantly and is ready as soon as it opens. The index uses about 70 MB per million files: a system disk with 2 million files comes to about 140 MB. With Prisma as administrator, it reads the NTFS table in seconds and updates the moment a file is created, renamed or deleted. Without administrator, or on a disk that is not NTFS, it walks through the folders, which takes longer, and updates every 15 minutes.

- **Pop-up:** Ctrl + Alt + Space opens a search box on top of any program, with up to eight results. Enter opens, Ctrl + Enter shows in folder, Ctrl + C copies the path and Esc closes. The arrows move through the list.
- **In the tab:** the full list, with name, folder, size and date. Click a column title to sort, again to reverse and a third time to go back to relevance. Double-click to open. Right-click opens, shows in folder or copies the path.
- **How to search:** case and accents don't matter, so "relatorio" finds "Relatório". Several words must all appear. Use quotes for a phrase with spaces, `ext:pdf;docx` for types, `folder:` for folders only, `file:` for files only, `*` and `?` as wildcards on the whole name, `!word` to exclude and a backslash, like `photos\2024`, to search the path.
- **Programs:** Start menu programs that match the text, up to three, appear at the top, including Microsoft Store ones. Enter opens, Ctrl + Shift + Enter opens as administrator and Ctrl + Enter shows in folder. On by default.
- **Web and AIs:** the text becomes a row to search in the default browser or ask an AI. If it looks like a question, because it ends with "?" or starts with how, what, why and the like, these rows come at the top; otherwise, after the first files. In Options, choose which ones appear: Google, Bing, DuckDuckGo and Brave Search, and the AIs Claude, ChatGPT, Gemini, Copilot, Perplexity, Grok and DeepSeek. Without a choice, Google and the installed AIs appear. An installed AI opens in its app; otherwise, the site opens. When the question can't be sent ready, it is copied and you just paste it with Ctrl + V. On by default.
- **Commands:** start with `>` to run a command, like `>ipconfig /all`. Run in PowerShell and Run in CMD appear: Enter runs without administrator, even with Prisma as administrator, and Ctrl + Shift + Enter runs as administrator, asking Windows for permission. The command opens in the Windows default terminal, which stays open. As you type, suggestions for commands, parameters and folders appear; Tab accepts the selected one or the first. The list comes from Windows itself the first time (PowerShell commands with their parameters, CMD commands with their description and the programs on the PATH), takes a few minutes and is saved. Off by default.
- **Complete like the terminal:** with commands on, a hidden PowerShell completes like the terminal, with paths, parameters and PowerShell history. It closes by itself after 2 idle minutes. Off by default.
- **Options:** choose the disks and the shortcut, what else appears in search, read the disks again or turn it off. Reading the disks again also rebuilds the command list. Turning off removes the index from memory and disk.
- **Disk with problems:** disks rated below Good in Disks and networks stay out, so the index doesn't strain a failing disk. You can include one by clicking it in Options.
- **Opening:** files open through Explorer, with the default program and without inheriting Prisma's administrator rights.

## Startup

Turns on, turns off, adds and removes what opens with Windows, and shows who signed each program. Sorts by name, enabled or disabled.

## Services

Shows the description of each service. You can start, stop and restart. The right mouse button opens properties in the Windows window itself.

## Uninstall

Removes programs in batches, creates a restore point, looks for leftovers and reports what was deleted.

- **Safety:** only exact name matches come checked. Similar names appear for you to review, folders go to the Recycle Bin and the registry gets a backup first.

## Windows tweaks

First an audit that only reads the PC. Then the Game, Development and Privacy profiles, in layers, with documented tweaks only. The Left out tab explains what stayed out and why.

- **You can:** undo. Each change keeps the previous value, and Undo only restores what nothing else touched afterwards. **Show log** shows each change, with before and after.
- **You can't:** expect more FPS: the gain is privacy and less noise. No profile touches updates, antivirus, UAC, Memory Integrity or virtualization. Removed apps come back through the Store, and that can fail.

## Tools

Brings together Windows repair (DISM then SFC), crash history, Windows Update, disk check, network, DNS, cleanup, winget, diagnostics and advanced restart. Each tool keeps its own step log, with the exact command, output, errors and the time of each step.

- **You can't:** use modes that write on a suspect disk. They stay blocked.

## Optimize

Suggests what to close before playing and always asks. Blacklist never closes, Whitelist always closes, and automatic detection suggests names for both lists. Shortcut: Ctrl + Alt + O.

If you want, a small question appears in the top right corner when a game opens, and leaves after 10 seconds. It comes off: turn it on in Power, in the When entering game mode box, with Ask whether to optimize.

## Power

Detects a game or high load and switches the power plan. It goes back to rest after 60 seconds without load. You choose the percentage and time that count as load.

A game is the program in focus that covers the screen and uses the GPU itself, or that lives in the Steam, Epic, GOG or Xbox game folders. Browsers, video players, remote access like Parsec, Discord, OBS and launchers never count, even in full screen.

- **You can:** edit a plan with every option Windows registers, including hidden ones, and import or export .pow files. The first edit keeps a copy of the original plan.
- **Limit:** Prisma's plans are created only in the installed version.

## Fullscreenizer

Makes the window in focus borderless fullscreen. Shortcut: Ctrl + Alt + F. Anything you use it on goes into a saved list, where you can check Automatic to apply it whenever the program opens.

- **You can't:** use it with a game in exclusive fullscreen. Put the game in windowed mode first.

## Sound

Controls the volume of each program. The keyboard volume keys, with a modifier (Ctrl by default), change only the program in focus. Prisma remembers the volume of each program.

- **You can:** choose the output and input device of each program, and add programs by hand.
- **Pin:** the pin next to the name keeps the program on the list even when closed, saving its volume, output and input. When it opens, it comes back at the saved volume.
- **Default device shortcuts:** each shortcut switches the Windows default output and input at once, for example Ctrl + Alt + Num 1 for the TV and the headset. **Add** creates a shortcut; pick the output and input (or Do not change) and click the field to record the keys. The X deletes it.
- **Keep the sound awake:** plays a stream with no audible sound, so HDMI and Dolby don't cut the start of the next sound. It stops on its own after 10 minutes without use.
- **Limit:** switching devices uses an internal Windows feature, and some programs only switch on reopening.

## HDR

A reading panel with the display mode, the active calibration profile, the brightness each source declares, what Windows tells games, the declared Dolby Vision and the accepted sound formats. It warns when something doesn't match.

- **Calibration:** opens fullscreen test patterns and saves one profile per TV picture mode.
- **Auto HDR per game:** forces, turns off or leaves it to Windows, in DX11 and DX12 games, without injecting anything.
- **Sound delay test:** plays a click with a flash, to compare with and without spatial sound.
- **You can't:** change what the TV does: the profile only informs Windows, and Windows may keep telling games the old values for a while. Direct reading of the display data exists only on NVIDIA cards.

## Capture

Takes photos with annotations and records video in AV1, HEVC or H.264. With the display in HDR, it records in SDR or HDR10. Video goes to the folder you choose.

- **Record Window and Record Screen:** Ctrl + Alt + R records the window in focus and Ctrl + Alt + T records the whole screen. With System sound on, it records every sound on the PC. Off, Record Window records only that window's sound.
- **Photo:** Ctrl + Alt + P selects an area, Ctrl + Alt + A takes the whole screen and Ctrl + Alt + J takes the window in focus. Enter or Ctrl + C copies, Ctrl + S saves, Ctrl + Z undoes and Esc cancels.
- **Replay:** in game mode, keeps the last seconds in memory. Ctrl + Alt + B saves.
- **You can't:** capture protected content, such as Netflix, which comes out black. H.264 has no 10 bits: recording in HDR10 uses HEVC. Recording requires a video encoder on the GPU.

## Share screen

In testing. Shows and controls another PC over the internet, for remote access or to play local co-op from a distance, with no account and no Prisma server. The picture comes from the graphics card with little delay, and the PC's sound comes along. The page has six tabs: **Share**, **Host**, **Viewer**, **Network**, **Shortcuts** and **Controllers**. With someone connected to this PC, a seventh one shows up, **Connected**.

- **Pair, once:** on the PC that will be seen, choose what whoever pairs can do and click **Create invite**. A window shows a short code, like `K7M2-QX9P`, that works once, for 10 minutes. In it, **Copy code** copies the code, **Copy link** copies a link and **Share** opens the Windows share window, to send it by email, Discord, WhatsApp and others. Whoever opens the link on a PC with Prisma sees Prisma open on the right page and ask whether to pair. On the other PC, you can also type the code under **Pair with another PC** and click **Pair**. Both show the same six-digit confirmation code: check it before accepting. When accepting, **Yes** trusts the device, and **No** pairs but asks on every connection.
- **Code and long invite:** the code is kept for 10 minutes on three public name relays (the same system iroh uses to find PCs), signed and with nothing that identifies you. If no relay answers, the window says so, and **Copy long invite** copies the full invite, which works without any relay.
- **Connect:** with sharing on at the host, click **Watch** on the device in the list. The connection tries the direct path first, then your VPN (Tailscale, ZeroTier) and, if nothing gets through, iroh's public relays. Everything is end-to-end encrypted.
- **Paired devices:** two tabs split the PCs this one watches from the ones watching this one. **Rename** gives the device another name, on this PC only; the original name stays below, smaller. Saving with the field empty goes back to the original.
- **Permissions per device:** sound, controller, keyboard, mouse, copy and paste (text, both ways) and secure screen. Tick and untick them in the list; if the device is connected, they apply right away. With none, the device only watches.
- **Host tab:** this PC's name for others, the monitor, the graphics card that encodes (the monitor's, or the other one on a laptop with two cards), the maximum resolution, frames per second, frame pacing (only when the screen changes, or constant, for a fixed rhythm), priority (lowest delay, balanced or quality), the codec (AV1, HEVC or H.264), the bandwidth limit, 10-bit HDR10 and 4:4:4. The automatic values follow the viewer. It also picks the sound that is sent (the whole PC or a single program), leaves the call app out of the sound (it then also leaves the list of sounds to send), limits how many people can join, keeps one person at a time on the mouse and keyboard, gives the host's mouse and keyboard priority and refuses connections while this PC is viewing another. Changes apply during the connection: picture changes restart the video for a moment, and the rest changes without restarting. Windows only allows leaving one program out of the sound: the call app that is playing is the one left out (Discord in a call, not Telegram open and silent), and the choice follows along during the connection.
- **Viewer tab:** open in borderless fullscreen, VSync, immersive mode (the mouse stays inside the window, and the Windows key and Alt + Tab go to the other PC), ask for 4:4:4, start muted, the latency meter (off, simple or detailed), the frames per second asked of the host (Automatic follows this screen's Hz), the graphics card that decodes and shows the picture (applies on the next connection) and the warning when the connection gets worse.
- **Meter and warning:** simple shows the end-to-end delay with a green, yellow or red dot. Detailed also shows the network, the video with the host's priority (lowest delay, balanced or quality), frames per second, bandwidth, decoding, lost frames and the route. The warning shows for a few seconds when the network loses more than 2% of frames, when latency varies by more than 10 ms or rises for 3 seconds in a row.
- **Connection menu:** Ctrl + Alt + Shift + P, in the viewer window, opens the menu with the connection's video, fullscreen, immersive mode (off, mouse, keyboard or both), frames per second, meter, warning, sound, VSync, ask for 4:4:4, controller in the background, keyboard as controller, Ctrl + Alt + Del and **Disconnect**, each with its shortcut beside it. The mouse and the arrows, Tab, Enter and Esc work. Everything applies right away; changing the frames per second or 4:4:4 restarts the video. If the host caps the frames per second, it sends the lower of the two.
- **Network tab:** a fixed UDP port, for people who forward the port on the router (empty means automatic), and asking the router to open the port by itself (UPnP, NAT-PMP, PCP). It also shows the route of the last connection.
- **Shortcuts tab:** changes the shortcuts for the menu, fullscreen, immersive mode, the meter, sound, Ctrl + Alt + Del, disconnecting and the emergency key. Click the shortcut and press the new combination; Esc cancels. The immersive mode shortcut turns it on and off only for the open connection; the mode that stays is the one on the Viewer tab.
- **Co-op and Controllers tab:** up to 4 people at once. Each person's controller becomes a virtual Xbox controller on the host. This needs ViGEmBus (Nefarius) or VIIPER installed on the host; Prisma installs neither. The controllers leave on their own when the game in front has an anti-cheat. The tab shows the controllers connected here, with battery, and the viewers' virtual ones. It also lets you send the controller while the window is in the background and use the keyboard as a controller.
- **HDR:** the host converts the HDR picture for viewers without an HDR screen. If you prefer, tick **Turn off HDR when the viewer has no HDR**: HDR comes back when the room empties, or the next time Prisma starts if something fails midway.
- **Windows service:** optional. With it, whoever has the **Secure screen** permission sees and uses the UAC prompt, the lock screen and the PIN screen, even after the PC restarts on its own. **Install** asks for administrator and shows what changes before installing. **Remove** undoes everything. The service runs a copy of Prisma, which is swapped for the new version when the updated Prisma opens as administrator. If the service keeps crashing, the page warns you.
- **Connected tab:** shows each device connected to this PC, with its route and latency. The access type changes right away and is saved for the next connections. **Disconnect** drops only that connection. **Ban** asks first, then drops and forgets the device, which only comes back with a new invite.
- **Notifications:** Windows shows a Prisma notification when someone connects to this PC or leaves. Clicking it opens the Connected tab.
- **Safety:** Ctrl + Alt + Shift + Q, on the host, disconnects everyone at once. **Forget** removes a device, which only comes back with a new invite.
- **It cannot:** work without Prisma open on the host (unless you use the service); capture protected content, which comes out black; hear the viewer's microphone.

## Widget

Shows CPU, GPU, RAM and VRAM on top of everything, in four styles. Drag it to move it. Shortcut: Ctrl + Alt + W.

## Settings

- **Start with Windows and administrator:** turn them on or off whenever you want. Changing this in the installed version asks for Windows permission.
- **Replace Task Manager:** turns it on or gives it back. Uninstalling also gives it back. Some Windows versions protect this change even from administrators. Then Prisma tells you and catches Ctrl+Shift+Esc itself while it is open, and the card shows "On through the shortcut". Ctrl+Alt+Del and the taskbar menu still open the Windows Task Manager. In the portable version you can only turn it off.
- **Force close:** a shortcut, Alt + F4 by default, ends the program in focus right away, even frozen in fullscreen. Anything unsaved is lost. The desktop, the taskbar and Windows processes keep the normal Alt + F4.
- **Restart Explorer:** choose a shortcut; there is none by default. Since it closes the taskbar, you press it twice within 4 seconds: the first press only warns you. The button next to the name restarts it right away, after asking.
- **Language:** Brazilian Portuguese, English, Spanish, Russian or Simplified Chinese. This guide and the version notes stay in English in the last three.
- **Updates:** see the section below.
- **Show the welcome screen on open:** brings the first screen back.

Each page has a **Default** button that resets the options of that page only.

## Updates

With automatic search on, Prisma looks for a new version once a day on GitHub, without sending your data. When you open the window and there is a new version, it asks if you want to update. Minimized or in the tray, it does not interrupt you.

- **Yes:** downloads, checks the file, replaces the program and restarts, with your settings. The installed version asks for Windows permission if it is not running as administrator.
- **No:** it asks again the next time the window opens.
- **Don't show again:** turns automatic search off. To turn it back on, check the option in Settings, which also has **Check now**.
- **What's new:** after an update, Prisma opens with what changed in each version since the one you had, even if you skipped several. To see it again, click **What's new** next to the version in Settings.

## Report a bug

Click the bug button at the top of the window. Choose the feature, the severity, describe what happened and add a photo if you want. The browser opens GitHub with everything written and you just confirm.

- **Privacy:** you can include PC data and a short log, without user name, PC name or file paths. The GitHub page is public, and you see everything before sending.
- **Crashes:** if Prisma crashes, it keeps a report in the data folder and tells you the next time it opens. The file stays on your PC only.

## Shortcuts

All of them can be changed in the program.

- **Ctrl + Alt + F:** Fullscreenizer
- **Ctrl + volume and mute keys:** volume of the program in focus
- **Ctrl + Alt + W:** Widget
- **Ctrl + Alt + O:** Optimize
- **Ctrl + Alt + Space:** search files, with search turned on
- **Alt + F4:** Force close
- **Ctrl + Alt + 1:** compact meter
- **Ctrl + Alt + 2:** full meter
- **Ctrl + Alt + 0:** hide the meter
- **Ctrl + Alt + 5:** meter controls
- **Ctrl + Alt + 3:** turn the limiter on or off
- **Ctrl + Alt + up or down arrow:** raise or lower the limit
- **Ctrl + Alt + 4:** data log
- **Ctrl + Alt + P, A and J:** photo of an area, the screen and the window
- **Ctrl + Alt + R and T:** record window and record screen
- **Ctrl + Alt + B:** save replay

Share screen shortcuts are changed in the page's Shortcuts tab: Ctrl + Alt + Shift + Q on the host disconnects everyone; in the viewer window, Ctrl + Alt + Shift + P opens the connection menu, Ctrl + Alt + Shift + F toggles fullscreen, Ctrl + Alt + Shift + I turns immersive mode on and off, Ctrl + Alt + Shift + M switches the meter, Ctrl + Alt + Shift + S turns sound on and off, Ctrl + Alt + End sends Ctrl + Alt + Del and Ctrl + Alt + Shift + D disconnects.

## General limits

- 64-bit Windows 11.
- No CPU temperature or power.
- Text that comes from Windows itself, such as service descriptions, stays in the Windows language.
- Everything Prisma changes on the system is recorded in acoes.log, in the data folder.
- The file has no digital signature, so Windows may warn about an unknown publisher.
- Share screen is still in testing: the internet connection uses iroh, a third-party library, and goes through its public relays when there is no direct path.

# Installation And Portable Mode

## Download

Version 0.2.0 is on the [v0.2.0 release page](https://github.com/aivrar/mediabuddy/releases/tag/v0.2.0):

| File | Use it for |
| --- | --- |
| [Media.Buddy_0.2.0_x64-setup.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/Media.Buddy_0.2.0_x64-setup.exe) | Installer. Recommended for most people. |
| [Media.Buddy_0.2.0_x64_en-US.msi](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/Media.Buddy_0.2.0_x64_en-US.msi) | MSI package, for environments that prefer MSI. |
| [mediabuddy.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/mediabuddy.exe) | Portable. No install; keeps its data beside the exe. |

## Upgrading From 0.1.0

Install or copy the new version over the old one. Your `data/` folder is kept.

The library database is upgraded in place the first time 0.2.0 opens it. Going
back to 0.1.0 afterwards is not supported: 0.1.0 does not keep caption origins
or topic positions up to date. Close Media Buddy and copy the `data/` folder
first if you may want to go back.

Captions saved by 0.1.0 show as "not recorded" under **Caption by**, because
0.1.0 did not track who wrote them. AI runs treat them as before.

## Portable Behavior

If you put `mediabuddy.exe` in a folder and run it, the app creates:

```text
data/
```

beside the exe. That folder stores settings, provider keys, downloads, logs,
model files, runtime files, thumbnails, and the SQLite database.

To move the app to another computer, copy both:

```text
mediabuddy.exe
data/
```

Sharing only the exe gives the other user a clean first-run app with no keys,
no downloads, no models, and no library.

If the folder containing the exe is not writable (for example a per-machine
install under `C:\Program Files`), Media Buddy stores its data in the per-user
folder instead:

```text
%LOCALAPPDATA%\MediaBuddy\data
```

The folder in use is written to the log at startup.

## Windows Requirements

- Windows 10 or 11.
- Microsoft Edge WebView2 Runtime.
- Internet access for provider searches and first-time model/runtime downloads.

Most Windows 10/11 systems already have WebView2 installed. If the app does not
start, install WebView2 Runtime from Microsoft.

## Build From Source

```powershell
npm install
npm run tauri dev
```

Create release builds:

```powershell
npm run tauri build -- --bundles nsis,msi
```

The build writes:

```text
src-tauri/target/release/mediabuddy.exe
src-tauri/target/release/bundle/nsis/Media Buddy_0.2.0_x64-setup.exe
src-tauri/target/release/bundle/msi/Media Buddy_0.2.0_x64_en-US.msi
```

## Data Directory Override

Set `MEDIABUDDY_DATA_DIR` before launch to use a custom data location:

```powershell
$env:MEDIABUDDY_DATA_DIR = "D:\MediaBuddyData"
.\mediabuddy.exe
```

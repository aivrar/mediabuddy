# Installation And Portable Mode

## Recommended Release Files

Publish the installer for normal users:

```text
src-tauri/target/release/bundle/nsis/Media Buddy_0.1.0_x64-setup.exe
```

Optional MSI installer:

```text
src-tauri/target/release/bundle/msi/Media Buddy_0.1.0_x64_en-US.msi
```

Portable executable:

```text
src-tauri/target/release/mediabuddy.exe
```

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

## Data Directory Override

Set `MEDIABUDDY_DATA_DIR` before launch to use a custom data location:

```powershell
$env:MEDIABUDDY_DATA_DIR = "D:\MediaBuddyData"
.\mediabuddy.exe
```

# Media Buddy

![License: MIT](https://img.shields.io/github/license/aivrar/mediabuddy)
![Platform: Windows](https://img.shields.io/badge/Platform-Windows%2010%2F11-blue)
![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-FFC131)
![Rust](https://img.shields.io/badge/backend-Rust-orange)
![SolidJS](https://img.shields.io/badge/frontend-SolidJS%20%2B%20TypeScript-2c4f7c)
![No Python](https://img.shields.io/badge/Python-not%20required-success)
![Stars](https://img.shields.io/github/stars/aivrar/mediabuddy)
![Issues](https://img.shields.io/github/issues/aivrar/mediabuddy)
![Last Commit](https://img.shields.io/github/last-commit/aivrar/mediabuddy)

**Portable Windows app to search, download, organize, preview, and AI-caption
stock photos and videos from Pixabay, Pexels, and Unsplash.**

Media Buddy is a native stock image downloader and local media library for
searching, downloading, organizing, previewing, tagging, and AI-captioning stock
photos and videos from Pixabay, Pexels, and Unsplash. It includes batch
downloads, provider metadata capture, a local REST API, and Florence-2 ONNX
computer vision for captions and object tags.

It is a Tauri 2 + SolidJS + Rust rewrite of the original Python ImageBuddy app.
There is no Python runtime requirement.

## Screenshots

| Search | Library | Settings |
| --- | --- | --- |
| <img src="docs/screenshots/search.png" alt="Search tab" width="320"> | <img src="docs/screenshots/library.png" alt="Library tab" width="320"> | <img src="docs/screenshots/settings.png" alt="Settings tab" width="320"> |

| Filters | Video Preview | API |
| --- | --- | --- |
| <img src="docs/screenshots/filters.png" alt="Search filters" width="320"> | <img src="docs/screenshots/video.png" alt="Video preview" width="320"> | <img src="docs/screenshots/api.png" alt="API tab" width="320"> |

## Highlights

- Multi-source search across Pixabay, Pexels, and Unsplash.
- Photo and video support, including preview posters for videos.
- Persistent search topics with per-provider cursors.
- Batch downloads with concurrency controls.
- Local library backed by SQLite WAL mode.
- Rich metadata capture where providers expose it.
- Large image/video inspector with editable caption and tags.
- Florence-2 ONNX AI vision for captions and object tags.
- Dynamic GPU/CPU worker planning for Florence-2.
- Built-in REST API for automation with bearer-token security.
- Portable data folder beside the executable.

## Download And Run

For normal users, publish the NSIS installer from:

```text
src-tauri/target/release/bundle/nsis/Media Buddy_0.1.0_x64-setup.exe
```

For portable testing, share:

```text
mediabuddy.exe
```

When launched, the portable executable creates a sibling `data/` folder for
settings, downloads, logs, the SQLite library, and Florence-2 model/runtime
cache.

## First Run

1. Open **Settings**.
2. Use **Get key** beside Pixabay, Pexels, and Unsplash to create provider API
   keys.
3. Paste each key and press **Test & save**.
4. Open **Images -> Search** and run a query.
5. Select result cards and choose **Download selected** or **Download all**.
6. Open **Images -> Library** to inspect, tag, delete, or AI-caption saved
   media.

## AI Vision

Florence-2 is not bundled in the executable. When you press **Load**, Media
Buddy downloads the required ONNX model and runtime files into:

```text
data/models/
```

GPU workers are used when compatible. CPU workers are used only for CPU mode or
as fallback when GPU planning/loading fails.

## REST API

The API tab starts a local REST server, defaulting to:

```text
http://127.0.0.1:5000
```

Use the API tab to copy the bearer token, open live docs, and copy example curl
commands. The status endpoint is public; other `/api/v1/*` routes require the
token when one is configured.

## Documentation

- Wiki pages: [docs/wiki/Home.md](docs/wiki/Home.md)
- PDF-ready manual: [docs/MEDIA_BUDDY_MANUAL.md](docs/MEDIA_BUDDY_MANUAL.md)
- PDF export notes: [docs/PDF_EXPORT.md](docs/PDF_EXPORT.md)
- Screenshot checklist: [docs/wiki/Screenshot-Checklist.md](docs/wiki/Screenshot-Checklist.md)
- Release checklist: [docs/wiki/Release-Checklist.md](docs/wiki/Release-Checklist.md)
- Release notes: [docs/RELEASE_NOTES_0.1.0.md](docs/RELEASE_NOTES_0.1.0.md)
- Security policy: [SECURITY.md](SECURITY.md)

## Build From Source

Requirements:

- Windows 10 or 11
- Rust stable
- Node.js 20+
- Microsoft C++ Build Tools / Visual Studio 2022 Build Tools
- WebView2 Runtime, usually already present on Windows 10/11

```powershell
# Clone this repository, then:
cd ImageBuddy
npm install
npm run tauri dev
npm run tauri build
```

Release executable:

```text
src-tauri/target/release/mediabuddy.exe
```

Installers:

```text
src-tauri/target/release/bundle/nsis/Media Buddy_0.1.0_x64-setup.exe
src-tauri/target/release/bundle/msi/Media Buddy_0.1.0_x64_en-US.msi
```

## Data Layout

```text
data/
|-- config/
|   `-- settings.json
|-- images/
|   |-- originals/
|   `-- thumbs/
|-- videos/
|   |-- originals/
|   `-- thumbs/
|-- logs/
|-- models/
|-- images.db
|-- images.db-shm
`-- images.db-wal
```

Override the data location with:

```powershell
$env:MEDIABUDDY_DATA_DIR = "D:\MediaBuddyData"
```

## Privacy

Media Buddy stores provider keys and the REST token locally in
`data/config/settings.json`. Downloads, metadata, logs, model files, and the
SQLite library stay local unless you share the folder yourself.

Search/download/API calls go to the configured providers and local REST API
clients. Florence-2 model/runtime downloads come from Hugging Face, Microsoft
ONNX Runtime, NuGet, and PyPI for cuDNN runtime files.

## License

MIT. See [LICENSE](LICENSE).

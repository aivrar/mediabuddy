# Media Buddy

A native Windows desktop app for searching, downloading, and managing stock
media (photos and videos) from Pixabay, Pexels, and Unsplash. Tauri 2 +
SolidJS frontend, Rust + axum backend.

Originally a Python (Tkinter) app called ImageBuddy; this is the rewrite that
removes Python entirely and ships as a single ~10 MB compiled `.exe`.

## What it does

- **Multi-source search** — Pixabay + Pexels + Unsplash in parallel, returns
  rich metadata (author, page URL, all URL tiers, view/like/download counts,
  AI-generated flag, blur hash, color, EXIF where available).
- **Photos and videos** — search either, or both. Videos download as `.mp4`
  with the provider's poster as the thumbnail.
- **Library** — SQLite (WAL mode) with URL deduplication and soft-delete
  blocking so deleted items aren't re-downloaded next time.
- **Batch operations** — multi-select then download, batch-delete, etc.
- **Live system footer** — CPU, RAM, and per-GPU stats (NVIDIA via NVML).
- **REST API** — built-in axum server with the same JSON shape as the
  original Python ImageBuddy API. 17 functional routes; 10 vision-related
  routes are stubbed pending the Florence-2 ONNX integration.
- **No Python runtime** — single `.exe`, ~10 MB.

## Build from source

Requires:
- Windows 10 / 11
- [Rust](https://rustup.rs/) (1.85+)
- [Node.js](https://nodejs.org/) (20+)
- Microsoft C++ Build Tools (Visual Studio 2022 Community or the standalone
  build tools)

```powershell
git clone <this-repo>
cd MediaBuddy
npm install
npm run tauri dev      # development with hot reload
npm run tauri build    # release build → src-tauri/target/release/mediabuddy.exe
```

Release builds use LTO + a single codegen unit (slower compile, smaller +
faster binary).

## Configuration

API keys for Pixabay / Pexels / Unsplash are entered in the **Settings** tab
and persisted to `data/config/settings.json`. The data directory lives next
to the `.exe` (or in development, under `src-tauri/target/debug/data/`). To
override the location, set `MEDIABUDDY_DATA_DIR`.

## Folder layout

```
data/
├── config/         settings.json, theme.json
├── images/
│   ├── originals/  full-resolution photos
│   └── thumbs/     300px JPEG thumbnails
├── videos/
│   ├── originals/  downloaded .mp4
│   └── thumbs/     poster JPEGs
├── logs/           reserved for file-based logs
├── models/         reserved for Florence-2 ONNX weights
├── images.db       SQLite database (WAL)
└── images.db-shm / images.db-wal
```

## REST API

Mounted in-process when the user starts it from the **API** tab (default
`127.0.0.1:5000`). All responses use:

```json
{ "success": true|false, "data": {...}, "error": "..." }
```

Full route list visible in the API tab. Highlights:

- `GET  /api/v1/status`
- `GET  /api/v1/stats`
- `GET  /api/v1/images?page=1&per_page=50&source=Pixabay&kind=video`
- `POST /api/v1/search` — `{ "query": "sunset", "sources": { "pixabay": 2 }, "kind": "both" }`
- `POST /api/v1/download` — single URL with metadata
- `POST /api/v1/download/batch` — async, returns `task_id`
- `GET  /api/v1/tasks/{task_id}` — async task status

## License

MIT.

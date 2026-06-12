# Getting Started

## First Run

1. Launch Media Buddy.
2. Open **Settings**.
3. Use **Get key** for Pixabay, Pexels, and Unsplash.
4. Paste each key and press **Test & save**.
5. Open **Images -> Search**.
6. Run a search, inspect results, and download selected items.
7. Open **Images -> Library** to browse downloaded media.

## Main Tabs

| Tab | Purpose |
| --- | --- |
| Images | Search, topics, downloads, library, image/video viewer, AI captioning. |
| Settings | Provider keys, theme, download behavior, API server, Florence-2. |
| Log | Search, download, API, and vision events. |
| API | Local REST server status, token, docs, examples, endpoints. |

## Basic Search Flow

1. Choose photo or video.
2. Choose providers.
3. Enter a query.
4. Set result count and safe-search options.
5. Press **Search**.
6. Double-click or inspect a card for a larger preview.
7. Download one item, selected items, or the full result set.

## Basic Library Flow

1. Open **Images -> Library**.
2. Filter by query, caption, author, tag, source, or kind.
3. Select an item to open the inspector.
4. Edit caption or tags.
5. Save metadata changes.
6. Run AI caption/tag tools when needed.

## Where Data Goes

Portable builds create a sibling `data/` folder beside the executable. Source
development builds use the app-specific data root resolved by the Tauri app.

Do not commit or publish `data/`.

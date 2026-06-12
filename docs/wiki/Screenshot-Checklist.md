# Screenshot Checklist

Use screenshots that show the app clearly without leaking secrets.

## Included

- Search tab with a real query, topic sidebar, result grid, and result
  inspector: `docs/screenshots/search.png`.
- Search filters and provider controls: `docs/screenshots/filters.png`.
- Library tab with a large image inspector and metadata panel:
  `docs/screenshots/library.png`.
- Library tab with a video item and playback controls:
  `docs/screenshots/video.png`.
- Settings tab showing provider key setup and Florence-2 controls with keys
  hidden: `docs/screenshots/settings.png`.
- API tab showing server state and endpoint list with tokens redacted:
  `docs/screenshots/api.png`.

## Still Optional

- Log tab with normal search/download/vision events.
- Florence-2 loaded state after a successful model load.

## Optional

- Batch download progress.
- AI caption controls in the library inspector.
- Provider quota row after searches.

## Redaction

Before publishing, confirm screenshots do not show:

- Provider API keys.
- REST bearer token.
- Private file paths with usernames.
- Private downloaded images or videos.
- Private logs.

## Suggested Filenames

```text
docs/screenshots/search.png
docs/screenshots/filters.png
docs/screenshots/library.png
docs/screenshots/video.png
docs/screenshots/settings.png
docs/screenshots/api.png
```

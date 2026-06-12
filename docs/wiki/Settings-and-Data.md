# Settings And Data

## Settings

Settings are stored locally:

```text
data/config/settings.json
```

Main settings include:

- Provider API keys.
- Theme.
- API host, port, auto-start, CORS, and token.
- Unsplash detail threshold.
- Florence-2 load mode and worker limits.

## Themes

The app includes multiple themes. Theme choice is local and can be changed from
Settings.

## API Server

Default:

```text
127.0.0.1:5000
```

Keep the API bound to loopback unless you intentionally need LAN access. If you
change the host, use a strong token.

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

## Backups

To back up a portable library, close Media Buddy and copy the entire `data/`
folder.

To reset the app, close Media Buddy and move or delete `data/`.

To reset only Florence-2 model/runtime cache, close Media Buddy and move or
delete:

```text
data/models/
```

## Logs

Logs are available in the Log tab and in the local data logs folder. Redact API
keys, bearer tokens, private file paths, and private media names before sharing
logs publicly.

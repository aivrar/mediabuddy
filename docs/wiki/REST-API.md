# REST API

Media Buddy includes a local REST API for automation.

Default base URL:

```text
http://127.0.0.1:5000
```

Open the API tab to view server status, copy the token, open live docs, and copy
curl examples.

## Authentication

`GET /api/v1/status` is public. Other `/api/v1/*` endpoints require the bearer
token when one is configured:

```text
Authorization: Bearer <token>
```

`PUT /api/v1/settings` rejects an empty `api_token`, an `api_host` that is not
an IP address, and an unknown `vision_execution_mode` with HTTP 400. When the
token is changed, the discovery file is updated straight away.

## Core Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/status` | Health check. |
| GET | `/api/v1/stats` | Library and disk stats. |
| GET | `/api/v1/docs` | Live route documentation. |
| GET | `/api/v1/openapi.json` | OpenAPI document. |
| GET | `/api/v1/settings` | Redacted settings. |
| PUT | `/api/v1/settings` | Patch settings. |
| POST | `/api/v1/api-keys/validate` | Validate and optionally save provider keys. |
| GET | `/api/v1/quota` | Provider quota snapshot. |
| GET | `/api/v1/logs` | Read app logs. |
| DELETE | `/api/v1/logs` | Clear app logs. |
| POST | `/api/v1/app/shutdown` | Shut down Media Buddy. |

## Library Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/images` | List media. |
| GET | `/api/v1/images/{id}` | Read one media row. |
| PUT | `/api/v1/images/{id}` | Update caption/tags. |
| DELETE | `/api/v1/images/{id}` | Delete one media item. |
| POST | `/api/v1/images/delete` | Batch delete. |
| GET | `/api/v1/images/{id}/file` | Serve original file. |
| GET | `/api/v1/images/{id}/thumb` | Serve thumbnail/poster. |
| POST | `/api/v1/images/query` | Query/filter media. |
| POST | `/api/v1/images/{id}/caption/restore` | Put back the caption an AI run replaced. |
| GET | `/api/v1/blocked` | Count of deleted items that are blocked. |
| DELETE | `/api/v1/blocked` | Clear the blocked list. |

Media rows include `caption_source` (`provider`, `ai`, `manual`, or empty when
there is no caption or its origin was never recorded).

A caption set with `PUT /api/v1/images/{id}` is recorded as `manual`. A client
that is not a person can say so by sending `caption_source` (`ai` or
`provider`) with the caption. `caption_source` can also be sent on its own to
label the caption an item already has, for example to claim a caption with no
recorded origin as `manual`. Any other value is rejected with HTTP 400.

`POST /api/v1/download` and `POST /api/v1/combo/download-analyze` accept the
same `caption_source` for the `alt` text they are given; it defaults to
`provider`.

Media rows also include `caption_previous` and `caption_previous_source`: the
caption an AI run last replaced, and its origin. `POST
/api/v1/images/{id}/caption/restore` swaps it with the current caption and
returns the row. It returns HTTP 409 when there is nothing to restore.

The file endpoints stream the file and honor `Range` requests (HTTP 206), so
large videos can be fetched in pieces.

Downloading a URL that was deleted from the library returns HTTP 409 with a
message saying it is blocked.

## Search And Download Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| POST | `/api/v1/search` | Multi-source search. |
| GET | `/api/v1/search/pixabay` | Single-source Pixabay search. |
| GET | `/api/v1/search/pexels` | Single-source Pexels search. |
| GET | `/api/v1/search/unsplash` | Single-source Unsplash search. |
| POST | `/api/v1/download` | Download one normalized result. |
| POST | `/api/v1/download/batch` | Download results as an async task. |
| GET | `/api/v1/tasks/{task_id}` | Poll async task status. |

Single-source searches return HTTP 502 with the provider's status when the
provider call fails, instead of an empty result. Finished tasks stay readable
for six hours (the most recent 200 are kept).

## Topic Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/topics` | List topics. |
| POST | `/api/v1/topics` | Create a topic. |
| PUT | `/api/v1/topics/{id}` | Rename/update a topic. |
| DELETE | `/api/v1/topics/{id}` | Delete a topic. |
| POST | `/api/v1/topics/{topic_id}/more` | Fetch next topic page. |
| POST | `/api/v1/topics/{topic_id}/reset` | Reset topic cursors. |
| GET | `/api/v1/topics/{topic_id}/images` | List saved library IDs for topic. |
| GET | `/api/v1/topics/{topic_id}/pending` | Fetched results not yet downloaded or dismissed. |
| DELETE | `/api/v1/topics/{topic_id}/pending` | Dismiss the pending results. |

Each entry in a `more` response's `progress` list has a `status` of `ok`,
`empty`, or `error`. `error` carries a `message` and means the provider call
failed; the cursor does not move, so the same page is fetched next time.

## Vision Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/vision/status` | Vision status. |
| POST | `/api/v1/vision/load` | Load Florence-2 workers. |
| POST | `/api/v1/vision/unload` | Unload workers. |
| POST | `/api/v1/vision/analyze/{image_id}` | Analyze one library image. |
| POST | `/api/v1/vision/analyze` | Analyze images by path or batch request. |

Analysis requests accept `caption_mode` (`missing`, `short`, `overwrite`,
`skip`), `caption_min_chars`, and two protections that both default to `true`:

- `protect_manual`: captions with `caption_source` `manual` are never replaced.
- `protect_unknown`: captions with an empty `caption_source` (origin never
  recorded) are never replaced.

The result reports `caption_written`, `caption_source`, `caption_previous`,
`tags_added`, and `caption_kept_reason`, which is null when the caption was
written and otherwise one of:

| Value | Meaning |
| --- | --- |
| `mode` | The caption mode leaves this caption alone. |
| `manual` | The mode would replace it, but it was typed by hand. |
| `unknown_origin` | The mode would replace it, but its origin was never recorded. |
| `tags_only` | The caption mode is `skip`. |
| `nothing_generated` | Florence-2 produced no caption text. |

Changed in 0.2.1: `overwrite` and `short` no longer replace captions with an
empty `caption_source` unless `protect_unknown` is `false`.

`POST /api/v1/vision/unload` returns HTTP 409 while an analysis job is running.

## Combo Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| POST | `/api/v1/combo/search-download` | Search then download. |
| POST | `/api/v1/combo/download-analyze` | Download one URL then analyze. |
| POST | `/api/v1/combo/analyze-unprocessed` | Analyze unprocessed library images. |
| POST | `/api/v1/combo/smart-analyze` | Auto-load, analyze, optionally unload. |
| POST | `/api/v1/combo/search-download-analyze` | Search, download, then analyze. |

## Example

```powershell
$token = "<token>"
$headers = @{ Authorization = "Bearer $token"; "Content-Type" = "application/json" }
$body = @{
  query = "manta ray"
  kind = "photo"
  sources = @{ pixabay = 5; pexels = 5; unsplash = 5 }
} | ConvertTo-Json

Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:5000/api/v1/search" -Headers $headers -Body $body
```

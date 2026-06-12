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

## Vision Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/vision/status` | Vision status. |
| POST | `/api/v1/vision/load` | Load Florence-2 workers. |
| POST | `/api/v1/vision/unload` | Unload workers. |
| POST | `/api/v1/vision/analyze/{image_id}` | Analyze one library image. |
| POST | `/api/v1/vision/analyze` | Analyze images by path or batch request. |

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

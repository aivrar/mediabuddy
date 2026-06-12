# Security Policy

## Supported Versions

Media Buddy is currently published as a `0.1.x` public preview.

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |
| older | No |

## Reporting A Vulnerability

Do not open a public issue with secrets, API keys, REST tokens, private file
paths, or private downloaded media.

Use GitHub private vulnerability reporting if it is enabled on the repository.
If it is not enabled, open a minimal issue that asks for a private contact path
without posting the sensitive details.

Useful report details:

- Media Buddy version.
- Windows version.
- Whether the app was run as portable exe, NSIS installer, or MSI installer.
- Steps to reproduce.
- Expected result and actual result.
- Relevant log lines with keys, tokens, and private paths removed.

## Local Secrets

Media Buddy stores provider API keys and the local REST API token in:

```text
data/config/settings.json
```

The `data/` folder is ignored by git and should never be committed. If a key is
accidentally shared, revoke or rotate it in the provider dashboard.

## Network Access

Media Buddy makes outbound requests to:

- Pixabay, Pexels, and Unsplash for searches and downloads.
- Hugging Face for Florence-2 ONNX model files when AI vision is loaded.
- Microsoft ONNX Runtime releases, NuGet, and PyPI for runtime DLLs when AI
  vision needs local inference dependencies.

The local REST API binds to `127.0.0.1` by default. If you expose it to another
host, use a strong token and understand that API clients can search, download,
edit metadata, run vision jobs, and shut down the app.

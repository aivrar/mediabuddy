# API Keys

Media Buddy searches Pixabay, Pexels, and Unsplash through official provider
APIs. Each provider key is configured separately.

## Get Keys

| Provider | Sign-up / Docs |
| --- | --- |
| Pixabay | https://pixabay.com/api/docs/ |
| Pexels | https://www.pexels.com/api/ |
| Unsplash | https://unsplash.com/developers |

Use the **Get key** button beside each provider in **Settings** to open the
right sign-up page.

## Save Keys

1. Open **Settings**.
2. Paste a key into the provider field.
3. Press **Test & save**.
4. If the provider returns success, the app saves the key immediately.

If a provider test fails because of network or quota behavior, press the same
test button again after a few seconds. A valid key can fail temporarily when a
provider is slow, rate-limited, or returning a transient error.

## Key Validation

The app validates each key against the matching provider. A Pixabay key should
not validate as a Pexels or Unsplash key. If a provider appears to accept the
wrong key, treat it as a bug and report it with logs redacted.

## Where Keys Are Stored

Keys are stored locally:

```text
data/config/settings.json
```

Do not publish this file.

## REST API Token

The app also creates a local REST API token. Use the API tab to copy it.

The status endpoint is public:

```text
GET /api/v1/status
```

Other `/api/v1/*` endpoints require:

```text
Authorization: Bearer <token>
```

Keep this token private if the API is reachable by anything outside your own
machine.

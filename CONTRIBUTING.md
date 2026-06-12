# Contributing

Thanks for helping improve Media Buddy.

## Development Setup

Requirements:

- Windows 10 or 11
- Node.js 20+
- Rust stable
- Microsoft C++ Build Tools or Visual Studio 2022 Build Tools
- WebView2 Runtime

```powershell
npm install
npm run tauri dev
```

## Quality Gates

Run these before opening a pull request:

```powershell
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
git diff --check
```

For release packaging:

```powershell
npm run tauri build -- --bundles nsis,msi
```

## Public Repo Rules

- Do not commit `data/`, downloaded media, model caches, logs, or API keys.
- Keep changes scoped to the problem being fixed.
- Prefer existing app patterns over new frameworks or broad rewrites.
- Add focused tests for backend behavior with risk or reusable contracts.
- Update docs when user-facing behavior changes.

## Pull Request Checklist

- Summary explains the user-facing change.
- Tests or validation steps are listed.
- Screenshots are included for UI changes.
- Secrets and local data were not committed.
- Release notes are updated when behavior changes.

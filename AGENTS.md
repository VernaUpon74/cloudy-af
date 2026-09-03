# Cloudy AF — agent conventions

## Version bumps

Whenever README, CHANGELOG, or docs are updated for a change, bump the app
version in the same commit. All of these must move together:

- `package.json`
- `sidecar/package.json` and `sidecar/package-lock.json` (2 spots)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`
- `flatpak/org.cloudy.af.appdata.xml` (add a `<release>` entry)
- `CHANGELOG.md` (retitle `## Unreleased` to `<version> — <date>`)

Cadence: patch for fixes (1.15.x), minor for user-facing features (e.g.
1.14.0, 1.16.0).

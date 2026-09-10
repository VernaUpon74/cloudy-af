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

## Flatpak release builds

`flatpak/org.cloudy.af-local.yml` does NOT compile the app — it installs the
Tauri binary prebuilt at `src-tauri/target/release/cloudy-af`. The binary
embeds the frontend `dist/` at compile time, so the exact release sequence is:

1. `npm run build` (host — rebuilds `dist/`)
2. `toolbox run -c arcticfox-build sh -c 'cargo build --release --offline --features tauri/custom-protocol'`
   from `src-tauri/`. The `--features tauri/custom-protocol` flag is
   MANDATORY (ADR-007): without it the binary is a dev-mode build that tries
   to load the frontend from `http://localhost:1420` and the flatpak shows
   `GDBus.Error:org.freedesktop.portal.Error.NotAllowed` at startup.
3. `flatpak-builder --disable-rofiles-fuse --repo=repo --force-clean build-dir flatpak/org.cloudy.af-local.yml`
   (`--disable-rofiles-fuse` is required on this system; plain runs fail at
   cleanup with a rofiles-fuse Permission denied error)
4. `flatpak build-update-repo repo`
5. `flatpak build-bundle repo builds/cloudy-af-<version>.flatpak org.cloudy.af master`
6. Reinstall: `flatpak uninstall --user org.cloudy.af` then
   `flatpak install --user builds/cloudy-af-<version>.flatpak`
   (the `af2-origin` bundle remote stays disabled; system installs need sudo
   we don't have).

Gotcha: if step 2 is skipped or run without the feature flag, step 3 silently
repackages the stale binary — the bundle gets new metadata with the OLD app.
Symptom: installed version looks updated but behavior/UI is outdated, plus the
portal error above.

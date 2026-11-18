# Releasing CaseControl

Releases are built, signed and published by CI from a version tag (ADR 0001 #39). Nothing is built or uploaded by
hand. Installed apps pick a release up through the updater (see "Updates" in `docs/architecture.md`).

## Cut a release

1. Bump the version in all three places (they must match the tag, CI checks it):
   - `Cargo.toml` (`[workspace.package] version`)
   - `apps/desktop/src-tauri/tauri.conf.json` (`version`)
   - `apps/desktop/package.json` (`version`)

   Then run `cargo check` so `Cargo.lock` picks up the new crate versions, and merge the change through a green PR.
2. Tag the merge commit on `main` with an annotated tag. The tag message becomes the release notes:

   ```sh
   git switch main && git pull
   git tag -a v0.2.0 -m "CaseControl 0.2.0" -m "What changed, one line per item."
   git push origin v0.2.0
   ```

## What CI does (`.github/workflows/release.yml`)

1. `version`: fails unless the tag (without `v`) equals the three versions above.
2. `checks`: runs `ci.yml` as a reusable workflow (frozen install, typecheck, frontend build, fmt, clippy, nextest,
   cargo-deny).
3. `release` (`windows-latest`, the only job with `contents: write`): `tauri-apps/tauri-action` runs
   `pnpm tauri build`, signs the updater artifacts with the `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, creates the GitHub release for the tag (published, not a draft,
   notes from the tag message) and uploads the NSIS installer, its `.sig` and `latest.json`.

Installed apps check `https://github.com/mkj777/CaseControl/releases/latest/download/latest.json` after the start and
every 6 hours, download the update silently and install it on the next quit or start. "Latest" is the newest
published, non-prerelease release.

## Signing key

- Private key: `%USERPROFILE%\.tauri\casecontrol-updater.key` (password protected), public key next to it
  (`.key.pub`). The public key is in `tauri.conf.json` (`plugins.updater.pubkey`); every installed app trusts only
  that key.
- CI secrets: `TAURI_SIGNING_PRIVATE_KEY` (the content of the private key file) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- Back up the private key and its password offline (e.g. password manager plus an offline copy). **If the key is
  lost, installed apps can never update again**: a new key means a new public key in the app, which only a manual
  reinstall delivers. Never commit the key.

## Rollback

A published release cannot be pulled back from installed apps, and the updater never downgrades. To undo a bad
release, publish a newer version (e.g. 0.2.1) with the fix or with the previous code; apps update to it as usual.
Users who need the old build right away can install the previous installer from its GitHub release by hand.
If a release run fails half-way, delete the partial release and the tag on GitHub, fix the cause and push the tag
again.

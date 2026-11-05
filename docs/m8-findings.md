# M8 findings: packaging

Date: 2026-11-05. tauri-cli 2.12.4. Plugins: tauri-plugin-updater 2.14, tauri-plugin-log 2 and tauri-plugin-process 2. The signer flags were checked with `npx tauri signer generate --help`.

## Signing (deferred)

- There is no paid Apple Developer account, so there is no Developer ID signing or notarization. `bundle.macOS.signingIdentity` is `"-"` (ad-hoc).
- The release build is signed `adhoc,runtime` (hardened runtime), per `codesign -dv`. It runs on the build machine and on Apple Silicon generally.
- A copy downloaded from GitHub is blocked by Gatekeeper at first launch. The README gives the **Open Anyway** steps and the `xattr -dr com.apple.quarantine` alternative.
- Adding Developer ID later needs no code: set `signingIdentity`, then pass the notarization credentials (`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`, or an API key) to `tauri build`.
- Keychain side effect: an ad-hoc signature changes with every build, so macOS may ask again whether the app can read its Keychain items (the Linear key and GitHub token) after an update. A self-signed or Developer ID certificate would fix that.

## Updater

- Tauri's updater checks files with its own minisign key pair, not Apple's signing.
- The key pair was made with `npx tauri signer generate -w ~/.tauri/humanlayer-local.key --ci -p ""`.
  - The **private key stays outside the repo**, in `~/.tauri/humanlayer-local.key` (mode 600). Back it up: losing it means installed copies can't verify new updates.
  - The public key is in `tauri.conf.json` (`plugins.updater.pubkey`).
- Feed: `https://github.com/teguhash/humanlayer-local/releases/latest/download/latest.json`.
  - The repo has no git remote yet, so this guesses the GitHub user from the git config. Change `plugins.updater.endpoints` if the repo lives elsewhere.
- `bundle.createUpdaterArtifacts: true` makes `tauri build` write `HumanLayer Local.app.tar.gz` and its `.sig` when `TAURI_SIGNING_PRIVATE_KEY` is set.
- `scripts/release.sh` builds a release and puts these in `target/release/github-release/` to attach to a GitHub Release tagged `v<version>`:
  - the `.dmg`
  - the `.app.tar.gz` and its `.sig`, renamed without spaces (GitHub rewrites spaces in asset names)
  - `latest.json`, with the version, date, platform `darwin-aarch64` or `darwin-x86_64`, signature and URL
- **Settings → Updates** checks the feed, then downloads and installs the update and restarts the app (`process:allow-restart`). With no release published yet, the check reports that it could not fetch the feed.
- Not tested yet: installing an update end to end. That needs two published releases.

## Logs and diagnostics

- tauri-plugin-log writes `~/Library/Logs/dev.teguh.humanlayerlocal/humanlayer-local.log` at Info level, plus stdout. Files rotate at 5 MB and the last 3 are kept.
- The app's and runtime's `eprintln!` calls are now `log::warn!`. Startup logs the version.
- **Settings → Diagnostics → Copy diagnostics** builds a plain-text report:
  - app version, macOS version and architecture, the data folder, the schema version, and row counts
  - each tool's version and path
  - whether a Linear key or GitHub token is saved and signed in, without account names
  - the last 200 log lines
- Before the report leaves the app, it is scrubbed:
  - tokens with known prefixes (`lin_api_`, `ghp_`, `gho_`, `github_pat_`, `sk-ant-`, `sk-`, and so on)
  - email addresses
  - long opaque strings (40 or more letters and digits)
  - the home folder path, which becomes `~`
- The text goes to the clipboard and is also shown, so it can be checked first.

## First-run checks

- `git`, `claude`, `codex` and `gh` are looked up on the login shell's PATH, the same one sessions get. Each one's `--version` runs with a 10-second timeout.
- `git` and `claude` are marked required. `codex` and `gh` are optional and show what they enable.
- At start, a banner lists missing tools: red if a required one is missing, otherwise blue. **How to install** opens **Settings → Tools**, which shows each tool's version, path and install command.

## Verified

- `cargo clippy --workspace --all-targets` is clean, and `cargo test --workspace` passes. That includes new tests for redaction and for the tool check against the real `git`.
- `TAURI_SIGNING_PRIVATE_KEY=… npm run tauri build` produced:
  - `HumanLayer Local_0.1.0_aarch64.dmg` (5.4 MB)
  - `HumanLayer Local.app.tar.gz` and its `.sig`
- The bundled app contains `rpi/.claude-plugin/plugin.json`, 22 skills and 7 agents.

# Caffettino

A tiny, whimsical macOS menu bar app that buys your Mac a coffee. Built with
[Rune](https://github.com/anomalyco/rune), with a sleepy cup that perks up when
your Mac is caffeinated.

- Click **Brew a little focus** to start; **Let it snooze** to stop.
- Pick a coffee break before starting: a saved preset (25 minutes and one hour
  at first) or **Until I stop**.
- Pick **+** to type any whole number of minutes (1–1440). Return brews it once;
  **Save as preset** (⌘S) keeps it. Up to three presets fit, always shortest
  first. **Remove** deletes the selected preset.
- Tab enters the selected duration; arrow keys change it, Tab from **+** reaches
  the minutes field, and Space presses a button.
- **Left-click the menu bar cup** to start or stop the selected break.
- **Right-click the cup** to pick a break, start/stop, open the window, or quit.
- Keyboard or VoiceOver activation of the menu bar cup opens the same menu.
- Closing the window keeps the app in the menu bar. Escape or ⌘W closes it;
  ⌘Q or **Quit Caffettino** quits and releases its sleep assertions.

## Run

Release installation (Apple Silicon, macOS 13+, available after the first release):

```sh
brew install --cask mattsverse/tap/caffettino
open /Applications/Caffettino.app
```

To build from source, use Rust 1.96+ with Cargo and Python 3.11+. No JavaScript
runtime is needed.

```sh
cargo run --locked
```

Build a standalone app (locally ad-hoc signed):

```sh
bash scripts/bundle.sh
open dist/Caffettino.app
```

You can move `dist/Caffettino.app` into Applications. This local build doesn't
include Developer ID signing or notarization for distribution.

Release Please creates version and changelog PRs from conventional commits.
Merging a release PR runs GitHub Actions: Developer ID signing, Apple
notarization, a DMG containing the app, and an automatic Homebrew cask update.
The plist in `assets/` is a template. Bundling inserts the release version from
`Cargo.toml` and the build number from `GITHUB_RUN_NUMBER` (or `1` locally)
before signing. The cask's quit instruction uses the template's bundle ID.

## How it works

Caffettino launches `/usr/bin/caffeinate -di -w <app-pid>`, adding `-t <seconds>`
for timed breaks. This keeps the display and idle system awake on battery or AC
power. It does not override lid closure, explicit Sleep, or the lock screen.
Stopping or quitting kills and reaps only its owned child. The `-w` parent watch
also releases assertions if Caffettino crashes or is force-quit. Existing
`caffeinate` processes are left alone. Breaks start off and aren't restored on
relaunch. The app doesn't install a login item.

Presets and the selected break are kept in
`~/Library/Application Support/com.matteogassend.caffettino/presets.json`:

```json
{
  "version": 1,
  "presets": [25, 60],
  "selected": 25
}
```

`presets` are minutes; `selected` is minutes, or `null` for **Until I stop**. A
selected time that isn't a preset is a one-off break, brought back in the **+**
field (never running). You can edit the file by hand, but quit Caffettino first:
it reads the file only at launch and rewrites it after each change. Durations
outside 1–1440, duplicates and presets past the three shortest are ignored. A
file that can't be read is left alone; Caffettino starts with the defaults and,
at its next change, keeps your file as `presets.json.bad`.

Rune is pinned to a specific pre-release revision in `Cargo.toml`. Its native
macOS tray backend is not implemented at that revision, so `tray-icon` supplies
the macOS status item and menu; the window, layout, drawing, interaction, and
accessibility use Rune. Native tray work runs on Rune's UI thread. Hidden
windows release their GPU resources; active breaks use a one-second health check.

## Verify

Run `mise install` once to install [lefthook](https://lefthook.dev) and its
git hooks. Before each commit that touches Rust files, the pre-commit hook
applies `cargo clippy --fix` and `cargo fmt`, re-stages the fixes, then blocks
the commit if Clippy still reports warnings.

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo run --locked -- --snap target/resting.png --tree target/resting.json
cargo run --locked -- --scene awake --snap target/awake.png
cargo run --locked -- --scene timed --snap target/timed.png
```

Headless Rune renders and UI tests simulate coffee breaks; they never launch a
tray icon or `caffeinate`. The macOS process tests briefly launch real children
to verify start/stop, restart, ownership, and cleanup.

Cargo denies Clippy's `pedantic`, `perf`, and `suspicious` lint groups. Both CI
and release builds run Clippy across all targets with warnings denied.

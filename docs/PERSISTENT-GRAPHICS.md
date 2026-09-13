# Persistent native graphics settings

## Implementation

Save frame limit, picture effects, HDR request and color accuracy when edited in
the native Graphics Settings panel. Restore them once before the native game
window appears, without requiring the panel to be opened. Device resets keep
the current settings. Reset effects saves neutral picture values; it does not
reset frame limit or display choices.

Use the macOS preferences suite `org.batesai.ffxi.graphics`. This gives the Wine
game process a stable domain independent of its executable name or bundle path.
Preferences belong to the macOS user and apply across game installs. Unsaved
frame/display values inherit the existing configuration. Unsaved picture fields
use neutral defaults. Persist the requested HDR value, not available headroom.

Picture and display groups are each stored as a property-list dictionary, with
versioned keys. Editing one group does not overwrite the others. Numeric values
are validated before use; picture bounds still use the existing clamping path.
Saving uses NSUserDefaults, which updates its cache immediately and persists
asynchronously through macOS. It does not rely on a graceful game shutdown.

## Rules check

- Work starts from the accepted picture-controls commits, excluding the shelved
  interpolation experiment.
- Use existing objc2 Foundation bindings and one initialization guard. No new
  dependency, environment variable, wire field or Clone/Copy derive.
- UI callbacks save; rendering never reads or writes preferences per frame.
- Native Foundation wiring stays in the native-host module. There are no new
  selectors, lint suppressions or shader changes.
- Keep the installed app and prior packages intact; build a separate candidate.
  No game launch or desktop control is part of this change. The user
  subsequently requested committing and pushing the work on its feature branch.

## Verification

Four focused native Foundation tests passed using Rust 1.97.1. They covered
restoring all picture values through a fresh defaults instance, unset versus
explicit zero/false, malformed values, group isolation, and saving neutral
values after reset. Each test used and cleaned up a separate temporary domain;
the user's graphics preferences were never read or written by the tests.

The production x86_64 renderer build passed. No shaders changed. No Wine or game
process was launched or stopped. A full game quit/relaunch check remains for
manual testing; the tests establish preference round trips, not gameplay.

## Candidate and rollback

The candidate is `packaged-persistent-graphics/FFXI-on-Mac.app` beside the accepted
`packaged-picture-controls` package. It excludes the shelved interpolation code.
The installed app remains unchanged. For manual testing, use local Docker LSB
with Hxitest, adjust values in Command-comma, quit, and launch the candidate again.
The values should already apply before reopening the panel.

Preferences are saved when edited in this new build. Values from an older
process that only held them in memory cannot be recovered after that process
exits. The first launch keeps existing defaults until the user saves choices.
To return to the old renderer, quit the candidate and run the installed app.
It ignores the new preferences; returning to this build reads them again.

## Save checkpoint

The user requested committing and pushing this candidate separately from the
shelved interpolation experiment. The pre-commit `make check` stopped at
`fmt-check` with formatting differences in the native picture-controls code,
including unchanged baseline files. Later check legs did not run. The full
`make test` suite was not run; the four preference tests and production build
above remain the available validation. This checkpoint does not claim the full
repository gates passed or promote the candidate to the installed baseline.

# Native game host

The opt-in `present.nativeHost` path places the existing Metal-backed Wine
game window inside a native top-level window as a borderless child. The host
owns the visible frame. Wine keeps its input event queue, content view, and
Metal layer. No screen capture, CPU pixel copy, or extra GPU submission is
introduced. Exclusive D3D fullscreen retains the existing path.

## Implementation and acceptance plan

1. Preserve a complete installed baseline and use a separate build and prefix.
2. Add the host with native lifecycle handling and an explicit fallback.
3. Verify both displays, resize, focus, mouse coordinates, minimize/restore,
   native fullscreen, device teardown, and repeated creation in a bounded probe.
4. Add a native settings entry point and useful live presentation controls.
5. Run the repository checks and local-server gameplay comparisons with the
   exact candidate artifacts. Do not infer latency from an FPS counter.
6. Obtain adversarial review and resolve findings before deployment.

## Rules check

- One config key, `present.nativeHost`, defaults off, with parser coverage and
  an entry in `mtld3d.conf`.
- One appended versioned operation controls host ownership. A capability bit
  in the existing device-info padding gates it, preserving the old attach ABI
  and safely disabling hosting when an older Unix library is loaded.
- Native window state belongs to the AppKit main thread. Its thread-local
  owner releases it at detach. No render-thread AppKit access is introduced.
- Existing Metal objects and Wine input dispatch are reused. No dependencies,
  environment variables, derive additions, or lint suppressions are introduced.
- One superclass minimization call requires raw dispatch because a typed method
  would reenter the override. The audit permits exactly that statement in that
  file; `docs/CONVENTIONS.md` records the exception.
- `make check`, `make test`, and the relevant conformance comparison remain
  the verification gates. Existing failures must be compared with baseline,
  never hidden by re-recording it.

## Controls and compatibility

The launcher exposes Native game window under the mtld3d renderer. It defaults
off and takes effect on the next launch. While playing:

- Command-comma opens the nonmodal Graphics Settings panel.
- Enter a frame limit and press Return to apply it immediately. Zero is uncapped.
- The frame limit applies for this process session and survives device Reset.
- Command-M minimizes, Command-W requests game close, and Control-Command-F
  toggles native fullscreen. The game can reject minimize and close requests.

The host keeps the original Wine child and Metal layer. It does not copy frames
or add a rendering pass. Post-processing and frame generation are not implemented.
Exclusive D3D fullscreen uses the existing Wine path. Older Unix libraries and
Wine drivers without the versioned contract fall back to ordinary presentation.
Unsupported MSAA and nonzero auto-depth formats are rejected before changing
window ownership. Later GPU allocation failures are not transactional and do
not promise restoration of the previous native-fullscreen state.

## Validation

The final production package passed an 18-stage bounded native probe on an LG
TV at scale 1 and a Retina display at scale 2. Checks cover image quadrants,
mouse coordinates, title-bar keyboard focus, real settings text input, live
30/60 FPS limits, canceled and accepted minimization, Wine-initiated minimize,
native fullscreen, rejected MSAA and depth Reset requests, exclusive/windowed
Reset, frame-limit persistence, canceled close and teardown.

Four lifecycle matrices each passed ten checkpoints: native hosting, disabled
hosting, an older Unix library, and a Wine driver without the host contract.
They cover both device-release orders, repeated creation and destroying the
Wine window before releasing the device. Those matrices preceded the final
typed initializer; the final 18-stage probe covers its construction/teardown.

The complete repository test run passed 2,155 tests across host-native and both
Windows architectures. After the final Reset validation change, all 42 selected
Reset tests passed across both PE architectures. Final formatting, clippy and
documentation checks passed. The aggregate check still reports five existing
audit findings in shader-cache documentation/tests and execute-policy prose;
it has no task-owned findings.

The upstream i686 device conformance check remains incomplete. Candidate and
preserved baseline both reached the 60-second bound with 14 failures and a
truncated result. Matching incomplete results do not establish conformance; the
checked-in baseline was not rewritten.

Packaged Hxitest gameplay with native hosting enabled completed world entry,
fixed-time Bastok Markets and Mines, and three resize/display moves at 4096 by
4096 background resolution. The test verified effective configuration, loaded
renderer/driver hashes, host attachment and restoration of saved files/settings.
The installed app was preserved during validation. After acceptance the candidate
was installed with the previous app retained for rollback. Three matched
on/off/on game runs completed. Markets measured 38.23/40.59/41.42 FPS and Mines
90.42/97.79/100.40 FPS. The first apparent penalty did not reproduce with the
unchanged native build; these bounded runs show no consistent penalty and do
not establish an improvement. Details are in the launcher acceptance report.

The launcher package has 15 installation/rollback tests, three package tests,
and ten harness tests passing. Ad-hoc package signature verification passed.
Developer ID notarization and long-session gameplay are not established by
these bounded local tests.

# Presentation recovery after hiding a window

Presentation must recover when a window returns from another macOS Space, even
if an occlusion notification is delayed or missed. A hidden window still submits
render work but skips drawable acquisition, since the compositor need not recycle
drawables for an off-screen surface.

The periodic display refresh now runs before the drawable gate, including while
presentation is suppressed. Its AppKit callback reconciles the live window's
occlusion state. Space membership and parent visibility are diagnostic only:
they must not suppress an exposed surface while a desktop transition settles.
The native host's occlusion events and workspace Space-change notifications also
refresh the current binding. Observers retain no window and resolve the binding
when called, so teardown and device reattachment remain safe.

Previously the periodic refresh was inside the successful-drawable branch, and
only the Wine child window's occlusion notification updated the hidden flag. A
stale hidden flag therefore disabled the refresh that could recover it. This
class of defect is confirmed by the controlled regression below. It does not
establish the cause of every reported freeze.

Existing GPU retirement and resource-lifetime synchronization is unchanged.
Drawable acquisition and GPU-retirement waits lasting at least 250 milliseconds
now emit warnings under `mtld3d::unix::present`, including sequence, duration and
hidden state. Visibility transitions log at info level. These diagnostics work
in production builds without frame tracing or per-frame log output.

## Controlled lost-notification regression

`scripts/native-host/visibility-recovery.py` launches its own standalone D3D9
probe in a disposable Wine SDK/prefix. It verifies the owned executable and
renderer mappings, resolves the hidden atomic from matching module symbols, and
injects one stale `true` value while the window remains visible. The helper
refuses a target whose parent is not the calling test runner. It never attaches
to an existing game. The run has a 90-second work deadline and bounded cleanup.

Compile `visibility-recovery.swift` with `swiftc`. The disposable Wine loader
copies need `com.apple.security.get-task-allow`, and the helper needs
`com.apple.security.cs.debugger`, applied through ad-hoc signing for this test.
These entitlements are not part of the game package. Build `probe.c` as the
standalone i686 D3D9 probe. Keep the SDK and prefix inside the output's parent.

```sh
python3 scripts/native-host/visibility-recovery.py \
  --wine /test/sdk/bin/wine --prefix /test/prefix \
  --probe /test/native-probe.exe --injector /test/visibility-recovery \
  --bundle /test/candidate/FFXI-on-Mac.app \
  --output /test/recovery-candidate --expect recover
```

Use `--expect stuck` with the preserved baseline. The baseline must remain hidden
for the four-second observation until the runner clears the injected state. The
candidate must clear it automatically and continue presenting. Output includes
the observed bit, presentation count, matching-module sample and renderer log.

Before packaging, use `make install-unix-x64 PROD=1` with a disposable `WINE_SDK`.
Cargo emits `libmtld3d_unix.dylib`; the Makefile stages `mtld3d.so` and its symbols.
Assert the staged `.so` matches the fresh dylib, then verify the packaged and
loaded hashes. A successful Cargo build alone does not update the staged `.so`.

## Acceptance record

The controlled baseline stayed hidden until manually cleared. The final
candidate recovered automatically and presented 240 frames during the same
four-second observation. Four windowed, four native-fullscreen and four focus
round trips passed image checks. Six local test-character desktop round trips
also resumed presentation, with all settings and test-owned processes restored.

All 2,161 repository tests passed (1,046 core/type, 207 native/shared, and 454
rendering tests per PE architecture). Both PE clippy legs, rustdoc, targeted
mechanical audits and whitespace checks passed. The combined check gate remains
blocked by existing formatting, native-clippy and audit findings outside this
change. The installed application and both preserved baseline copies are unchanged.

Ordinary baseline desktop switching did not reproduce the original long freeze,
so natural reproduction remains a validation limitation. The controlled test
confirms recovery from a stale flag; it does not prove every freeze shares that
cause. Physical monitor dragging is not newly validated by these Space tests.

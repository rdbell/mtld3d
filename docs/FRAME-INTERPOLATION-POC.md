# MetalFX frame interpolation experiment, revision 2

## Disposition: shelved at the user's request

Stop this experiment. Do not schedule further runs, promote either candidate,
or include the experimental renderer in the accepted baseline. The user judged
frame interpolation unlikely to be viable without substantially more work.

The first prototype's manual feedback was warping/trails, alternating sharpness,
and uneven motion. Revision 2 matched the image resolutions and changed pacing;
it was built and packaged, but no separate v2 runtime result or measured
improvement was established in this conversation before the experiment stopped.
Do not record v2 as either a proven fix or a measured failure.

The practical blockers remain reliable scene depth and motion, UI/effect handling,
and validated presentation cadence. Resolution matching alone does not solve
these. Revisit only after a new user request and a materially different input or
presentation approach that addresses the recorded limitations. Do not repeat the
same flat-depth/block-search prototype as a new optimization experiment.

Keep the source on the experiment branch and both candidate packages for reference.
The native picture-controls app in /Applications remains the accepted baseline;
no rollback, process control, configuration change or cleanup was performed when
shelving the experiment. The build and testing instructions below are historical.

This session-only experiment defaults to off. The first manual test reported
warping, sharpness flicker and uneven motion. Revision 2 addresses the resolution
mismatch and changes presentation timing. Motion and depth quality remain open.

## What changed

Real and generated images now come from matching private textures, bounded to
1280 by 720. Both use the same final scaling shader. This makes the whole image
softer than normal full-resolution presentation, instead of deliberately
alternating a 720p image with a full-resolution image. MetalFX can still filter
its generated image differently. Turning interpolation off restores ordinary
full-resolution presentation.

The renderer now waits for GPU completion before requesting drawable presentation.
The callback never waits for a timer, drawable or GPU operation. A regular host-time
timeline replaces deadlines derived independently from each CPU frame. Late pairs
advance to the next available source slot; they do not bunch up to catch up.
The timeline begins with one source period of headroom, adding about one source
frame of buffering. It is not an input-latency improvement.

The cached display refresh ceiling determines compatible target rates. Examples
with no lower user cap:

| Display ceiling | Real-frame target | Presentation target |
| --- | --- | --- |
| 120 Hz | 60 FPS | 120 FPS |
| 60 Hz | 30 FPS | 60 FPS |
| 144 Hz | 36 FPS | 72 FPS |

The output uses an integer number of refresh periods and the source is capped at
60. A lower existing frame cap may reduce these targets further. Combinations
below approximately 30 source FPS suspend the experiment. Unknown refresh uses
a logged 60/120 assumption. This is not display-link phase locking, and a
variable-refresh display may behave differently from its reported ceiling.
Actual display timing still needs measurement during manual testing.

## Inputs and limits

MetalFX receives consecutive private SDR snapshots. A bounded GPU block search
estimates current-to-previous motion. The search is unchanged in revision 2.

Depth remains a **synthetic flat plane**, with virtual camera near/far 0.1/1000,
vertical FOV 60 degrees, and conventional Z at 0.5. These are explicit approximations,
not recovered FFXI geometry. No separate UI mask, object motion, disocclusion ground
truth or camera-cut signal is available. Text, nameplates and effects may still warp
or leave trails. Poor results do not establish how MetalFX would behave with correct
engine inputs.

HDR suspends interpolation without changing the HDR setting. The motion search
covers roughly 32 pixels at the interpolation resolution. Fast turns and scene
cuts can defeat it. Toggle off/on to reset history after an abrupt transition.
This experiment does not increase the game's rendered FPS.

## Timing evidence

After a few seconds, reopen Graphics Settings to see the median and 95th percentile
displayed-frame intervals, late frames, and long gaps. This summary refreshes on
source-frame reports, roughly every two seconds. It is unavailable until there
are displayed samples.

The `mtld3d::interpolation` log target records:

- Source, encoded, requested and confirmed-displayed counts separately.
- Actual interval minimum, median, p95 and maximum from the latest 240 samples.
- Deadline error, short intervals, long gaps and displayed content reversals.
- Skipped source slots, completed-but-stale pairs, and callback delivery reordering.
- Selected resolution, display ceiling, source/output targets and unknown-refresh
  assumptions. Debug logging includes requested and actual times for each drawable.

Samples are sorted by actual presented time before measuring intervals, because
callback delivery order is not necessarily display order. Only callbacks with a
positive finite presented time count. These callbacks are compositor evidence;
no external measurement of physical display scanout has been performed.

At a steady 120 presentations per second, intervals should cluster around 8.33 ms.
At 60 they should cluster around 16.67 ms. A large gap between median and p95, short
intervals, or rising skipped slots indicates that pacing or throughput still needs
work. FPS totals alone cannot establish smooth motion.

## Ownership and fallback

The existing three-drawable pool bounds pending presentation. A completion-owned
retain keeps each drawable alive until it is requested or discarded. Snapshot
copies finish before presentation is requested, so later reuse of the input
textures cannot overwrite a displayed drawable.

Mode, picture, geometry, queue and display changes reset history. New sessions
invalidate old completion callbacks. GPU failures stop interpolation. A failed
second-drawable acquisition falls back instead of repeating its timeout every
frame. Frames older than 100 ms at completion and out-of-order completed pairs
are discarded. Already requested drawables can remain queued briefly during a
mode change. PE resource-retirement counters keep their original meaning.

## Rules check

- Session atomics, resource ownership and two short timing locks stay in the Unix
  presentation path. The timing samples are bounded to 240. No worker, dependency,
  PE wire field or runtime configuration key is introduced.
- Existing typed objc2 APIs own MetalFX and drawable operations. There are no new
  raw selectors, Clone/Copy derives or lint suppressions.
- Existing fullscreen copy and pipeline creation code is reused. Normal D3D draws,
  depth storage and the disabled presentation path remain independent of the POC.
- The accepted installed app and both prior packages remain available. No commit,
  push, installation or launch was requested for this revision.
- The user's no-testing instruction takes precedence over the repository's normal
  test gates. Only a production build and package integrity checks are performed.

## Manual testing, historical instructions

Use `packaged-frame-interpolation-v2/FFXI-on-Mac Frame Interpolation v2.app`
beside the preserved first candidate in `packaged-frame-interpolation`. The accepted
baseline remains in `packaged-picture-controls` and `/Applications`.

Use local Docker LSB with Hxitest only. Open Graphics Settings with Command-comma,
turn HDR off, and enable the interpolation experiment. Begin on a 120 Hz display
in a scene that can render at least 60 FPS without interpolation. Inspect slow
pans and fine text for sharpness alternation. Reopen Settings after a few seconds
and record its interval statistics. Then try a more demanding scene and watch for
rising gaps or skipped slots in the logs.

The generated-only checkbox displays interpolated images at source cadence for
artifact inspection. It intentionally does not double the presentation rate.
Turning interpolation off restores the normal full-resolution image.

The launcher uses the existing game installation and prefix when Play is clicked.
WINEDLLPATH selects the Unix renderer from this candidate bundle; prefix shims are
unchanged from the baseline. Quit the candidate and launch the installed app to
return to the accepted renderer. Packaging does not launch either app or register
the candidate with Launch Services.

## Validation status

The production x86_64 Rust build passed. Source review covered matched snapshots,
callback retention, cancellation, scheduling serialization, bounded resource use
and cadence statistics. No tests, capability probes, game launches or runtime
measurements were performed by the agent for revision 2. Visual improvement and
actual display cadence await the user's manual test.

## Archive checkpoint

The user subsequently requested committing and pushing all work. This saves the
experiment on `feature/frame-interpolation-poc`; it does not resume testing or
promote either package. The build-time source patch remains unchanged. The
committed report includes the later shelving decision.

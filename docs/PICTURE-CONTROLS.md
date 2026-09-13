# Native picture controls

Command-comma opens Graphics Settings when the native game window is enabled.
The controls apply to the current process session. Closing the panel or
resetting the D3D device preserves them; restarting the game restores the
startup configuration and neutral effects. No launcher settings are rewritten.

| Control | Range and initial value | Behavior |
| --- | --- | --- |
| Adaptive sharpening | 0 to 1, initially 0 | Local-contrast-dependent sharpening with neighborhood bounds to limit halos. |
| Exposure | -2 to +2 stops, initially 0 | Linear-light brightness multiplier. |
| Contrast | 0.5 to 1.5, initially 1 | Encoded-space contrast around middle gray. |
| Saturation | 0 to 2, initially 1 | Linear-light luminance-based color adjustment. |
| Temperature | -1 to +1, initially 0 | Approximate cool/warm balance, not a calibrated Kelvin control. |
| Bloom strength | 0 to 1, initially 0 | Bright-region glow with a normalized screen blend. |
| Bloom threshold | 0 to 1, initially 0.75 | Encoded brightness threshold, decoded to linear light for extraction. |
| Bloom radius | 1 to 8, initially 3 | Blur footprint at quarter source resolution. |
| FXAA | Initially off | Directional edge smoothing on the finished image. |
| HDR output | Inherits `hdr.enable` | Requests the existing SDR-to-HDR mapping when the display has EDR capability. |
| Accurate sRGB colors | Inherits `color.space` | Tags source primaries as sRGB so macOS converts them for the display; off preserves the existing display-matched vivid policy. |

Reset effects restores sharpening, color controls, FXAA and bloom to neutral.
It leaves the frame limit and HDR/color-space choices alone. These controls are
session overrides like the existing frame-limit field, not new config-file keys.
HDR availability is reported separately from the requested checkbox state.
HDR converts the game's SDR output; it cannot recover already clipped detail.
Color accuracy describes source tagging, not display calibration. For faithful
SDR output, enable accurate colors, disable HDR, and keep effects neutral.

## Rendering and ownership

The optional chain runs after game rendering and before the existing
stretch/MetalFX/HDR presentation route:

1. Optional FXAA at source resolution.
2. Optional quarter-resolution bright extraction and two separable blur passes.
3. Adaptive sharpening, bloom composite and color adjustment at source resolution.
4. Existing scaling and HDR conversion to the drawable.

FXAA alone uses one pass. Other adjustments share one composite pass. Bloom
adds three small passes. All effects apply to the scene and game UI together;
FXAA can soften text and bloom can illuminate bright nameplates and menus.
The separate software cursor receives the HDR/color-space policy, not scene
effects. Sharpening before downsampling may be less visible at high background
resolutions than it is at 1:1 display resolution.

Neutral settings take one atomic load and retain the original presentation
path, with no picture shader compilation, allocation or extra render pass.
Only actual presentation calls the effect chain. Game textures, readbacks and
readback helpers do not change. Final SDR output remains within [0, 1] for the
existing HDR mapper. Source alpha is preserved.

The optional pipeline library is compiled lazily and caches failures. Failure
leaves normal presentation available and emits a bounded warning. Compilation
can cause a hitch on first enable; its duration has not been measured.

Scratch textures are exclusive to one command buffer until its completion
handler runs. The handler is registered before encoding, so partial failures
also retain their resources. This supports multiple in-flight frames and
command queues without reusing textures while the GPU still accesses them.
Up to three completed scratch sets are cached; additional in-flight leases
follow the existing submission lifetime and retire without a new CPU wait.
Resize discards idle old geometries. Disabling all effects releases idle
textures and lets active leases retire. Bloom uses two RGBA16Float buffers at
quarter resolution. FXAA plus composite needs two full-size BGRA8 buffers;
FXAA alone and composite alone need one. At 4096-square source resolution,
all-on scratch storage is about 144 MiB per lease before driver overhead;
three cached sets are about 432 MiB. Memory and frame-time costs require
manual measurement.

The main-thread panel publishes one coherent settings snapshot. Submission
never accesses AppKit controls. HDR changes reuse retained bound-layer access
and select the rendering pipeline from each drawable's actual format. Color
policy changes force a layer retag even when HDR mode stays the same; the
software cursor tracks that revision and redraws its current sprite.

## Rules check

- Unix-only session state and renderer resources; no ABI, capability, shader-cache
  schema, environment-variable or dependency changes. Existing objc2 features
  are enabled for buttons, cells and sliders.
- The settings mutex protects a small copied per-frame value, and the scratch
  mutex protects exclusive leases. No UI lock spans GPU submission or completion.
- `Picture` is Copy for coherent snapshots and `PicturePipelines` is Copy for
  process-lifetime pipeline handles. Both are recorded in the derive inventory.
- The additional OnceLock needs the runtime Metal device and separately caches
  optional shader failure. It does not affect the existing presentation cache.
- One target/action selector token pairs with a typed local callback. The exact
  site, signature and ownership rationale are documented in CONVENTIONS and
  constrained in the selector audit. No new raw message sends or lint allows.
- This is optional presentation processing, not a D3D render-state heuristic or
  game-specific shader rewrite. Existing HDR and color-policy code is reused.

## Validation status and manual acceptance

The user reported an excellent manual playtest and explicitly accepted this
build as the new baseline on 2026-09-13 UTC. The production x86_64 Unix renderer
built successfully with Rust 1.97.1; package signatures and recorded hashes
were verified. The PE/Unix ABI is unchanged, so the package reuses the existing
PE DLLs. The accepted signed renderer SHA-256 is:

`99fd358a792617c2c1fe6a22df9eab37b32a352e45900569fcc87aeef03fef04`

Acceptance authorizes installing the exact tested bundle. No source or runtime
changes are part of promotion. The earlier instruction against automated tests
and agent-driven gameplay remains in force; those checks were not run. The
manual report does not specify a full effects/display matrix or measured FPS.
The standalone Metal compiler is unavailable in the installed developer tools;
picture shaders compile lazily in the game. No independent shader compilation
or performance results are claimed.

The following remains a suggested regression checklist, not a record of
completed checks:

1. Preserve the current installed baseline and build the candidate. Run the
   repository checks before deployment; do not infer compilation from source review.
2. Use Hxitest on the local Docker LSB server. Check neutral/off first, including
   minimap, text, menus, camera movement and spell effects.
3. Change each slider and FXAA independently. Confirm immediate visible changes,
   useful endpoints, reset-to-neutral behavior and unchanged mouse coordinates.
4. Compare all-off with reset after all-on. Check neutral sharpness/color and
   FPS, and inspect first-enable shader errors in the log.
5. Toggle HDR and accurate colors independently on SDR and HDR displays. Move
   between displays, resize, minimize/restore and use native fullscreen. Inspect
   cursor matching, brightness transitions, edges and signs of stale frames.
6. Stress rapid effect changes and repeated resize. Inspect stability and memory
   after disabling effects. Compare frame times at 1:1 and 4K background resolution.
7. Exercise a device Reset and reopen the panel. Session choices must survive;
   a new process must return to startup defaults.

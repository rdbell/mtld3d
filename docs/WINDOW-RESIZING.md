# Window resizing

Windowed D3D9 applications own their rendering resolution. Resizing or moving
the native window changes the presentation destination; it does not authorize
the renderer to replace the back buffer, implicit depth surface, viewport, or
scissor. Only the application's explicit Reset changes that state.

The cursor window procedure used to recreate those surfaces on WM_SIZE. A
fixed-resolution application then drew its original image into a larger
surface, leaving unused space, or into a smaller one, clipping its output.
Removing that implicit reset lets the existing Metal presentation path stretch
the complete image to the window. Fullscreen coverage and cursor handling stay
in their existing paths.

This preserves the original aspect only when the window has the same aspect.
Arbitrary window proportions stretch the image, as D3D9 Present specifies.
It does not make an old game relayout its interface or change its internal
resolution while running.

References:

- [D3D9 Present](https://learn.microsoft.com/en-us/windows/win32/api/d3d9/nf-d3d9-idirect3ddevice9-present)
- [D3D9 Reset](https://learn.microsoft.com/en-us/windows/win32/api/d3d9/nf-d3d9-idirect3ddevice9-reset)

## Rules check

This removes an implicit resource-recreation path and adds a regression in the
existing implicit-surface suite. It adds no runtime configuration, static state,
wire fields, dependencies, derives, or lint suppressions. Presentation scaling,
fullscreen handling, and explicit Reset retain their existing implementation.
The suite coverage index and resource-lifetime comments follow the change.

## Validation

The standalone four-quadrant probe reproduces unused space after enlargement
and incorrect proportions after shrinking with the previous renderer. The
production candidate preserves the requested 1280 x 720 back buffer through
six window size and display transitions. Pixel checks pass on the 1x external
display and the 2x built-in display.

The implicit-surface regression checks surface dimensions, viewport and scissor
across enlargement, shrink, zero-size, and restoration messages, followed by an
explicit Reset. `make test` passes all 2,154 tests: 1,248 host tests and 453
end-to-end tests on each PE architecture.

Formatting, clippy and documentation builds pass. `make check` stops at the
repository audit, whose five findings are identical on the unchanged development
baseline: three documentation-shape findings, an inline test module, and a
comment mentioning a competing renderer. No new audit finding was added.

The checked-in conformance baseline comes from a different Wine build. A fresh
i686 comparison of the installed renderer and the production candidate reports
identical visual results (236 failures), state-block results (zero failures),
and D3D9Ex results (one expected failure). The device subtest hits the 120-second
limit on both. It reaches different points before termination (23 versus 34
failures), so device conformance remains inconclusive. No baseline file was
rewritten, and this is not a claim that the full conformance gate passes.
The failed i686 gate prevents the aggregate target from reaching x86_64.

Local game validation passed with Hxitest on local Docker LSB. Six native
window transitions covered enlargement, shrink, both backing scales, a
move between displays without resizing, and restoration to the original size.
The complete scene and UI remained visible. The loaded DLL hash matched the
production candidate. The harness restored 44 file states and launcher
preferences and left no related processes running.

The installed application now contains that production DLL. Its code signature
was verified, and a complete known-good application copy was retained before
installation. The launcher's renderer installer copies the updated DLL into the
game directories on the next Play. The x87 sidecars and Wine runtime were not
changed. Source changes remain on `fix/window-resize`.

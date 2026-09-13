//! Opt-in MetalFX experiment with approximate inputs and bounded dual presentation.

use core::ptr::NonNull;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CACurrentMediaTime, CAMetalDrawable, CAMetalLayer};

use super::{command, macdrv};

mod gpu;
mod timing;

static ENABLED: AtomicBool = AtomicBool::new(false);
static PREVIEW: AtomicBool = AtomicBool::new(false);
static REVISION: AtomicU64 = AtomicU64::new(1);
static STATE: Mutex<Option<Session>> = Mutex::new(None);
static STATUS: Mutex<&'static str> = Mutex::new("Interpolation is off.");
static SOURCE: AtomicU64 = AtomicU64::new(0);
static GENERATED: AtomicU64 = AtomicU64::new(0);
static SUBMITTED_REAL: AtomicU64 = AtomicU64::new(0);
static SUBMITTED_GENERATED: AtomicU64 = AtomicU64::new(0);
static DISPLAYED_REAL: AtomicU64 = AtomicU64::new(0);
static DISPLAYED_GENERATED: AtomicU64 = AtomicU64::new(0);
static PANEL_HZ: AtomicU64 = AtomicU64::new(0);
static CADENCE_STATUS: Mutex<String> = Mutex::new(String::new());
static GPU_FAILED: AtomicBool = AtomicBool::new(false);

struct Session {
    resources: Option<gpu::Resources>,
    revision: u64,
    queue: usize,
    layer: usize,
    source_size: (usize, usize),
    drawable_size: (usize, usize),
    last_source: Instant,
    next_source: Instant,
    period: f64,
    pacer: Arc<timing::Pacer>,
    seeded: bool,
    last_report: Instant,
    report_counts: [u64; 3],
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pacer.invalidate();
    }
}

pub fn settings() -> (bool, bool) {
    (
        ENABLED.load(Ordering::Acquire),
        PREVIEW.load(Ordering::Acquire),
    )
}

pub fn configure(enabled: bool, preview: bool) {
    PREVIEW.store(preview, Ordering::Release);
    ENABLED.store(enabled, Ordering::Release);
    GPU_FAILED.store(false, Ordering::Release);
    if !enabled {
        CLEANUP.store(true, Ordering::Release);
    }
    invalidate();
    CADENCE_STATUS.lock().unwrap().clear();
    set_status(if enabled {
        "Requested; waiting for a supported SDR frame."
    } else {
        "Interpolation is off."
    });
    log::info!(target: "mtld3d::interpolation", "requested={enabled} generated_only={preview}; approximate motion and flat depth");
}

/// Publish resets without making the AppKit thread wait for Metal or a drawable.
pub fn invalidate() {
    REVISION.fetch_add(1, Ordering::Release);
}

pub fn detach() {
    invalidate();
    if let Ok(mut state) = STATE.try_lock() {
        *state = None;
    }
}

pub fn status() -> String {
    format!(
        "{}\n{}",
        *STATUS.lock().unwrap(),
        *CADENCE_STATUS.lock().unwrap()
    )
}

/// Published by the existing main-thread display refresh; never query AppKit here.
pub fn set_display_hz(hz: f64) {
    let hz = if hz.is_finite() && hz > 0.0 { hz } else { 0.0 };
    if PANEL_HZ.swap(hz.to_bits(), Ordering::AcqRel) != hz.to_bits() {
        invalidate();
    }
}

fn source_period(minimum: f64) -> f64 {
    let panel = f64::from_bits(PANEL_HZ.load(Ordering::Acquire));
    let desired = 2.0 / minimum.max(1.0 / 60.0);
    if panel <= 0.0 {
        return 2.0 / desired;
    }
    // An integer number of display refreshes per output avoids a 120-on-144
    // beat pattern. Variable refresh and exact vblank phase still need measurement.
    2.0 * (panel / desired).ceil().max(1.0) / panel
}

fn set_status(message: &'static str) {
    let mut status = STATUS.lock().unwrap();
    if *status != message {
        *status = message;
        log::info!(target: "mtld3d::interpolation", "{message}");
    }
}

/// Return true only when this path scheduled the supplied real drawable.
///
/// The caller owns the command buffer and commits it after PE retirement handlers
/// are installed. All work uses that buffer's ordered queue. No worker, extra
/// submission or PE lifetime counter is introduced.
pub fn present(
    cb: &ProtocolObject<dyn MTLCommandBuffer>,
    source: &ProtocolObject<dyn MTLTexture>,
    layer: &CAMetalLayer,
    real: &ProtocolObject<dyn CAMetalDrawable>,
) -> bool {
    if !ENABLED.load(Ordering::Acquire) {
        // Discard optional resources on the first off-frame only. Most baseline
        // frames return after the atomic branch in the caller.
        *STATE.lock().unwrap() = None;
        CLEANUP.store(false, Ordering::Release);
        return false;
    }
    let sequence = SOURCE.fetch_add(1, Ordering::Relaxed) + 1;
    let target = real.texture();
    let minimum = macdrv::min_present_duration_sec();
    if target.pixelFormat() != MTLPixelFormat::BGRA8Unorm || minimum > 1.0 / 30.0 {
        *STATE.lock().unwrap() = None;
        set_status("Suspended: requires SDR and a frame limit of at least 30.");
        return false;
    }
    if GPU_FAILED.load(Ordering::Acquire) {
        *STATE.lock().unwrap() = None;
        set_status("Disabled after a GPU error. Toggle off/on to retry.");
        return false;
    }
    let revision = REVISION.load(Ordering::Acquire);
    let interval = source_period(minimum);
    if interval > 1.0 / 29.0 {
        *STATE.lock().unwrap() = None;
        set_status("Suspended: refresh/cap combination is below 30 source FPS.");
        return false;
    }
    let queue = &*cb.commandQueue() as *const _ as *const () as usize;
    let layer_id = core::ptr::from_ref(layer) as usize;
    let source_size = (source.width(), source.height());
    let drawable_size = (target.width(), target.height());
    let mut slot = STATE.lock().unwrap();
    let rebuild = slot.as_ref().is_none_or(|state| {
        state.revision != revision
            || state.queue != queue
            || state.layer != layer_id
            || state.source_size != source_size
            || state.drawable_size != drawable_size
            || (state.period - interval).abs() > 0.000001
    });
    if rebuild {
        CADENCE_STATUS.lock().unwrap().clear();
        let size = interpolation_size(drawable_size);
        let resources = match gpu::Resources::new(&cb.device(), size) {
            Ok(resources) => Some(resources),
            Err(message) => {
                set_status(message);
                None
            }
        };
        log::info!(target: "mtld3d::interpolation", "v2 matched_size={}x{} panel_hz={} source_target_fps={:.3} output_target_fps={:.3} unknown_refresh_assumption={}",
            size.0, size.1, f64::from_bits(PANEL_HZ.load(Ordering::Acquire)), 1.0 / interval,
            2.0 / interval, PANEL_HZ.load(Ordering::Acquire) == 0);
        let now = Instant::now();
        *slot = Some(Session {
            resources,
            revision,
            queue,
            layer: layer_id,
            source_size,
            drawable_size,
            last_source: now,
            next_source: now,
            period: interval,
            pacer: timing::Pacer::new(revision, interval, PREVIEW.load(Ordering::Acquire)),
            seeded: false,
            last_report: now,
            report_counts: counts(),
        });
    }
    let state = slot.as_mut().expect("session initialized");
    if state.resources.is_none() {
        return false;
    }

    let now = Instant::now();
    if let Some(wait) = state.next_source.checked_duration_since(now) {
        // At most one source interval; never wait on a backlog or the GPU here.
        std::thread::sleep(wait.min(Duration::from_secs_f64(interval)));
    }
    let now = Instant::now();
    let gap = now.duration_since(state.last_source).as_secs_f64();
    let reset = !state.seeded || gap > 0.100;
    let dt = now
        .duration_since(state.last_source)
        .as_secs_f32()
        .clamp(1.0 / 1000.0, 0.100);
    state.last_source = now;
    let next = state.next_source + Duration::from_secs_f64(interval);
    state.next_source = if next > now {
        next
    } else {
        now + Duration::from_secs_f64(interval)
    };
    let source_time = CACurrentMediaTime();
    let completion = RcBlock::new(move |ptr: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        // SAFETY: Metal supplies the completed live buffer for this callback.
        let buffer = unsafe { ptr.as_ref() };
        if buffer.status() == MTLCommandBufferStatus::Error {
            GPU_FAILED.store(true, Ordering::Release);
            invalidate();
            log::error!(target: "mtld3d::interpolation", "GPU work failed; interpolation disabled: {:?}", buffer.error());
        }
    });
    // SAFETY: the buffer copies the callback; it owns no game or AppKit pointers.
    unsafe { cb.addCompletedHandler(RcBlock::as_ptr(&completion)) };
    let Some(images) = state
        .resources
        .as_mut()
        .expect("resources checked")
        .encode(cb, source, dt, reset)
    else {
        state.resources = None;
        state.pacer.invalidate();
        set_status("Interpolation encoding failed; ordinary presentation is active.");
        return false;
    };
    state.seeded = true;
    if !reset {
        GENERATED.fetch_add(1, Ordering::Relaxed);
    }
    let preview = PREVIEW.load(Ordering::Acquire);
    // The first frame only initializes history, so its generated output is not
    // counted or displayed. MetalFX still sees it as its previous encoded frame.
    let extra = if !reset && !preview && layer.maximumDrawableCount() >= 3 {
        layer.nextDrawable()
    } else {
        None
    };
    if !reset && !preview && extra.is_none() {
        state.resources = None;
        state.pacer.invalidate();
        set_status("No spare drawable; ordinary presentation is active. Toggle to retry.");
        return false;
    }
    let show_generated = !reset && (preview || extra.is_some());
    // Main-thread toggles never block on the renderer's state lock. Honor a
    // changed mode before queuing presentation; the next frame rebuilds history.
    if REVISION.load(Ordering::Acquire) != revision {
        state.seeded = false;
        return false;
    }
    let real_source = if preview && show_generated {
        &*images.generated
    } else {
        &*images.real
    };
    if !command::encode_present_copy(cb, real_source, &target) {
        state.seeded = false;
        state.pacer.invalidate();
        state.resources = None;
        set_status("Presentation copy failed; ordinary presentation is active.");
        return false;
    }
    let mut generated = None;
    if let Some(extra) = extra {
        let extra_target = extra.texture();
        if extra_target.pixelFormat() == target.pixelFormat()
            && (extra_target.width(), extra_target.height()) == drawable_size
            && command::encode_present_copy(cb, &images.generated, &extra_target)
        {
            generated = Some(extra);
        } else {
            state.seeded = false;
            set_status("Generated drawable skipped after a display change or copy failure.");
        }
    }
    timing::enqueue(
        cb,
        timing::Pair {
            real,
            generated: generated.as_deref(),
            real_is_generated: preview && show_generated,
            sequence,
            source_time,
            pacer: Arc::clone(&state.pacer),
        },
    );
    if state.seeded {
        set_status(if reset {
            "Warming interpolation history."
        } else if preview {
            "Generated-only preview; approximate inputs."
        } else if show_generated {
            "Matched 720p pair; approximate motion/depth."
        } else {
            "Real frames only: no spare drawable for interpolation."
        });
    }
    report(state);
    // Keep normal display/headroom and cursor ownership reconciliation ticking.
    macdrv::current_headroom();
    macdrv::poll_capture_from_present();
    true
}

/// Visit once to release resources after disabling; otherwise only read atomics.
pub fn should_visit() -> bool {
    ENABLED.load(Ordering::Acquire) || CLEANUP.load(Ordering::Acquire)
}

static CLEANUP: AtomicBool = AtomicBool::new(false);

fn interpolation_size(size: (usize, usize)) -> (usize, usize) {
    // Integer arithmetic keeps geometry bounded, preserves aspect to 8 pixels,
    // and guarantees a nonempty motion grid even for a tiny resized window.
    let width = size.0.clamp(8, 16384);
    let height = size.1.clamp(8, 16384);
    let scaled = if width <= 1280 && height <= 720 {
        (width, height)
    } else if width * 720 > height * 1280 {
        (1280, height * 1280 / width)
    } else {
        (width * 720 / height, 720)
    };
    ((scaled.0 / 8 * 8).max(8), (scaled.1 / 8 * 8).max(8))
}

fn counts() -> [u64; 3] {
    [
        SOURCE.load(Ordering::Relaxed),
        DISPLAYED_REAL.load(Ordering::Relaxed),
        DISPLAYED_GENERATED.load(Ordering::Relaxed),
    ]
}

fn report(state: &mut Session) {
    let elapsed = state.last_report.elapsed().as_secs_f64();
    if elapsed < 2.0 {
        return;
    }
    let current = counts();
    let rates: Vec<_> = current
        .iter()
        .zip(state.report_counts)
        .map(|(now, before)| {
            f64::from(u32::try_from(now.saturating_sub(before)).unwrap_or(u32::MAX)) / elapsed
        })
        .collect();
    log::info!(target: "mtld3d::interpolation",
        "source_fps={:.1} displayed_real_fps={:.1} displayed_generated_fps={:.1} encoded={} submitted_real={} submitted_generated={} displayed_real={} displayed_generated={}",
        rates[0], rates[1], rates[2], GENERATED.load(Ordering::Relaxed),
        SUBMITTED_REAL.load(Ordering::Relaxed), SUBMITTED_GENERATED.load(Ordering::Relaxed), current[1], current[2]);
    *CADENCE_STATUS.lock().unwrap() = state.pacer.summary();
    state.last_report = Instant::now();
    state.report_counts = current;
}

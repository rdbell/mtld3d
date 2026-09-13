//! GPU-ready presentation and a bounded record of actual drawable display times.

use core::ptr::NonNull;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDrawable};
use objc2_quartz_core::{CACurrentMediaTime, CAMetalDrawable};

static EPOCH: AtomicU64 = AtomicU64::new(0);
const MAX_SAMPLES: usize = 240;
const READY_MARGIN: f64 = 0.001;

/// Each session owns a separate clock, including callbacks still in flight.
pub struct Pacer {
    epoch: u64,
    revision: u64,
    period: f64,
    preview: bool,
    state: Mutex<Clock>,
    presentation: Mutex<()>,
}

struct Clock {
    next_first: f64,
    last_sequence: u64,
    skipped_slots: u64,
    stale_pairs: u64,
    callback_reorders: u64,
    max_display_time: f64,
    samples: Vec<Sample>,
}

struct Sample {
    time: f64,
    deadline: f64,
    order: u64,
}

impl Pacer {
    pub fn new(revision: u64, period: f64, preview: bool) -> Arc<Self> {
        Arc::new(Self {
            epoch: EPOCH.fetch_add(1, Ordering::AcqRel) + 1,
            revision,
            period,
            preview,
            presentation: Mutex::new(()),
            state: Mutex::new(Clock {
                next_first: 0.0,
                last_sequence: 0,
                skipped_slots: 0,
                stale_pairs: 0,
                callback_reorders: 0,
                max_display_time: 0.0,
                samples: Vec::new(),
            }),
        })
    }

    pub fn invalidate(&self) {
        // Only this clock may invalidate its epoch. Dropping an old session
        // after a replacement was created must not cancel the new session.
        let _ = EPOCH.compare_exchange(
            self.epoch,
            self.epoch + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn current(&self) -> bool {
        self.epoch == EPOCH.load(Ordering::Acquire)
            && self.revision == super::REVISION.load(Ordering::Acquire)
            && super::ENABLED.load(Ordering::Acquire)
            && !super::macdrv::window_occluded()
    }

    /// Sorted timestamps distinguish callback delivery order from display order.
    pub fn summary(&self) -> String {
        let mut clock = self.state.lock().unwrap();
        clock.samples.sort_by(|a, b| a.time.total_cmp(&b.time));
        if clock.samples.len() < 2 {
            return "Awaiting displayed-frame timing.".into();
        }
        let expected = if self.preview {
            self.period
        } else {
            self.period * 0.5
        };
        let mut intervals: Vec<_> = clock
            .samples
            .windows(2)
            .map(|pair| pair[1].time - pair[0].time)
            .collect();
        intervals.sort_by(f64::total_cmp);
        let p50 = intervals[intervals.len() / 2] * 1000.0;
        let p95 = intervals[(intervals.len() - 1) * 95 / 100] * 1000.0;
        let late = clock
            .samples
            .iter()
            .filter(|sample| sample.time - sample.deadline > expected * 0.5 + 0.001)
            .count();
        let short = intervals
            .iter()
            .filter(|&&value| value < expected * 0.5)
            .count();
        let gaps = intervals
            .iter()
            .filter(|&&value| value > expected * 1.5 + 0.001)
            .count();
        let reversed = clock
            .samples
            .windows(2)
            .filter(|pair| pair[1].order < pair[0].order)
            .count();
        let max_error = clock
            .samples
            .iter()
            .map(|sample| (sample.time - sample.deadline).abs())
            .fold(0.0_f64, f64::max)
            * 1000.0;
        log::info!(target: "mtld3d::interpolation",
            "cadence epoch={} samples={} expected_ms={:.3} min_ms={:.3} p50_ms={p50:.3} p95_ms={p95:.3} max_ms={:.3} late={late} short={short} gaps={gaps} display_reversals={reversed} max_deadline_error_ms={max_error:.3} skipped_slots={} stale_pairs={} callback_reorders={}",
            self.epoch, clock.samples.len(), expected * 1000.0, intervals[0] * 1000.0,
            intervals[intervals.len() - 1] * 1000.0, clock.skipped_slots, clock.stale_pairs, clock.callback_reorders);
        format!(
            "Display intervals p50/p95: {p50:.1}/{p95:.1} ms; late {late}/{}; gaps {gaps}.",
            clock.samples.len()
        )
    }
}

pub struct Pair<'a> {
    pub real: &'a ProtocolObject<dyn CAMetalDrawable>,
    pub generated: Option<&'a ProtocolObject<dyn CAMetalDrawable>>,
    pub real_is_generated: bool,
    pub sequence: u64,
    pub source_time: f64,
    pub pacer: Arc<Pacer>,
}

/// Retain the drawables until GPU completion, then request distinct display times.
///
/// This never waits on the completion thread. The three-drawable layer pool
/// bounds pending work. A revision or geometry change invalidates old pairs.
pub fn enqueue(cb: &ProtocolObject<dyn MTLCommandBuffer>, pair: Pair<'_>) {
    let pending = Mutex::new(Some(Pending {
        real: Drawable::retain(pair.real),
        generated: pair.generated.map(Drawable::retain),
        real_is_generated: pair.real_is_generated,
        sequence: pair.sequence,
        source_time: pair.source_time,
        pacer: pair.pacer,
    }));
    let complete = RcBlock::new(move |ptr: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        let Some(pair) = pending.lock().unwrap().take() else {
            return;
        };
        // SAFETY: Metal lends a live completed command buffer for this callback.
        let buffer = unsafe { ptr.as_ref() };
        if buffer.status() != MTLCommandBufferStatus::Completed {
            return;
        }
        pair.present();
    });
    // SAFETY: Metal copies the block. Its RAII owners also release uncommitted pairs.
    unsafe { cb.addCompletedHandler(RcBlock::as_ptr(&complete)) };
}

struct Pending {
    real: Drawable,
    generated: Option<Drawable>,
    real_is_generated: bool,
    sequence: u64,
    source_time: f64,
    pacer: Arc<Pacer>,
}

impl Pending {
    fn present(self) {
        let _presentation = self.pacer.presentation.lock().unwrap();
        let now = CACurrentMediaTime();
        let mut clock = self.pacer.state.lock().unwrap();
        if !self.pacer.current()
            || self.sequence <= clock.last_sequence
            || now - self.source_time > 0.100
        {
            clock.stale_pairs += 1;
            log::debug!(target: "mtld3d::interpolation", "discarded completed pair seq={} age_ms={:.3}", self.sequence, (now - self.source_time) * 1000.0);
            return;
        }
        clock.last_sequence = self.sequence;
        let earliest = now + READY_MARGIN;
        // Start with one source period of headroom so small completion jitter
        // does not immediately miss the first slot on every following frame.
        let mut first = if clock.next_first > 0.0 {
            clock.next_first
        } else {
            earliest + self.pacer.period
        };
        if first < earliest {
            // Advance by complete source periods to keep a stable phase. This
            // cannot accumulate a backlog, or compress two frames into one slot.
            let missed = Duration::from_secs_f64(earliest - first)
                .as_nanos()
                .div_ceil(Duration::from_secs_f64(self.pacer.period).as_nanos());
            let skipped = u32::try_from(missed).unwrap_or(u32::MAX).max(1);
            first += f64::from(skipped) * self.pacer.period;
            clock.skipped_slots += u64::from(skipped);
            // Duration rounds to nanoseconds; preserve the not-before boundary.
            if first < earliest {
                first += self.pacer.period;
                clock.skipped_slots += 1;
            }
        }
        let real_time = if self.generated.is_some() {
            first + self.pacer.period * 0.5
        } else {
            first
        };
        // A warm-up real occupies a real-frame slot. The next midpoint belongs
        // halfway to the following real, not a whole source interval later.
        clock.next_first = if self.generated.is_none() && !self.pacer.preview {
            first + self.pacer.period * 0.5
        } else {
            first + self.pacer.period
        };
        // Keep scheduling serialized, but release the timing lock before calling
        // Metal: a presented callback must be free to acquire it immediately.
        drop(clock);
        if let Some(generated) = self.generated {
            request(&generated, first, true, self.sequence * 2, &self.pacer);
        }
        request(
            &self.real,
            real_time,
            self.real_is_generated,
            self.sequence * 2 + 1,
            &self.pacer,
        );
    }
}

fn request(drawable: &Drawable, deadline: f64, generated: bool, order: u64, pacer: &Arc<Pacer>) {
    let retained_pacer = Arc::clone(pacer);
    let shown = RcBlock::new(move |ptr: NonNull<ProtocolObject<dyn MTLDrawable>>| {
        // SAFETY: Metal lends this drawable for the callback's duration.
        let time = unsafe { ptr.as_ref() }.presentedTime();
        if time <= 0.0 || !time.is_finite() {
            return;
        }
        if generated {
            super::DISPLAYED_GENERATED.fetch_add(1, Ordering::Relaxed);
        } else {
            super::DISPLAYED_REAL.fetch_add(1, Ordering::Relaxed);
        }
        let mut clock = retained_pacer.state.lock().unwrap();
        if time < clock.max_display_time {
            clock.callback_reorders += 1;
        }
        clock.max_display_time = clock.max_display_time.max(time);
        clock.samples.push(Sample {
            time,
            deadline,
            order,
        });
        if clock.samples.len() > MAX_SAMPLES {
            clock.samples.sort_by(|a, b| a.time.total_cmp(&b.time));
            clock.samples.remove(0);
        }
        log::debug!(target: "mtld3d::interpolation", "displayed order={order} generated={generated} time={time:.6} requested={deadline:.6} error_ms={:.3}", (time - deadline) * 1000.0);
    });
    let object = drawable.get();
    // SAFETY: the live retained drawable copies the block, which owns its clock.
    unsafe { object.addPresentedHandler(RcBlock::as_ptr(&shown)) };
    if generated {
        super::SUBMITTED_GENERATED.fetch_add(1, Ordering::Relaxed);
    } else {
        super::SUBMITTED_REAL.fetch_add(1, Ordering::Relaxed);
    }
    object.presentAtTime(deadline);
}

/// A unique retain on a Metal drawable, with no AppKit thread affinity.
struct Drawable(usize);

impl Drawable {
    fn retain(drawable: &ProtocolObject<dyn CAMetalDrawable>) -> Self {
        // SAFETY: the borrowed object is live and this creates our independent retain.
        let retained = unsafe { Retained::retain(core::ptr::from_ref(drawable).cast_mut()) }
            .expect("live drawable");
        Self(Retained::into_raw(retained) as usize)
    }

    fn get(&self) -> &ProtocolObject<dyn CAMetalDrawable> {
        // SAFETY: this owner holds the retain until Drop; the borrow cannot escape it.
        unsafe { &*(self.0 as *const ProtocolObject<dyn CAMetalDrawable>) }
    }
}

impl Drop for Drawable {
    fn drop(&mut self) {
        // SAFETY: retain transfers exactly one owned reference into this unique owner.
        drop(unsafe { Retained::from_raw(self.0 as *mut ProtocolObject<dyn CAMetalDrawable>) });
    }
}

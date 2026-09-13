//! macOS persistence for the native graphics panel, outside the rendering loop.

use std::sync::{Once, atomic::Ordering};

use objc2::{AllocAnyThread, rc::Retained, runtime::AnyObject};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSNumber, NSString, NSUserDefaults};

use super::{super as macdrv, FRAME_LIMIT};
use crate::metal::picture::{self, Picture};

const SUITE: &str = "org.batesai.ffxi.graphics";
const PICTURE: &str = "picture.v1";
const DISPLAY: &str = "display.v1";
const FRAME: &str = "frameLimit.v1";
static RESTORED: Once = Once::new();

fn open() -> Option<Retained<NSUserDefaults>> {
    let result = NSUserDefaults::initWithSuiteName(
        NSUserDefaults::alloc(),
        Some(&NSString::from_str(SUITE)),
    );
    if result.is_none() {
        log::warn!(target: "mtld3d::preferences", "Cannot open graphics preferences; changes apply to this run only.");
    }
    result
}

/// Restore before hosting the window; later device resets keep live overrides.
pub fn restore(mtm: MainThreadMarker) {
    RESTORED.call_once(|| {
        let Some(defaults) = open() else { return };
        picture::update(read_picture(&defaults));
        if let Some(limit) = read_frame(&defaults) {
            FRAME_LIMIT.store(limit, Ordering::Relaxed);
            let pacing = macdrv::unpack_pacing(macdrv::PRESENT_PACING_BITS.load(Ordering::Relaxed));
            macdrv::set_display_sync_enabled(mtld3d_shared::MetalHandle::NULL, &pacing);
        }
        let (hdr, accurate) = read_display(&defaults);
        if hdr.is_some() || accurate.is_some() {
            let current = macdrv::session_display();
            macdrv::set_session_display(mtm, hdr.unwrap_or(current.0), accurate.unwrap_or(current.1));
        }
        log::info!(target: "mtld3d::preferences", "Restored native graphics preferences from {SUITE}.");
    });
}

pub fn save_picture() -> bool {
    let Some(defaults) = open() else { return false };
    write_picture(&defaults, picture::snapshot());
    true
}

pub fn save_display(hdr: bool, accurate: bool) -> bool {
    let Some(defaults) = open() else { return false };
    write_group(
        &defaults,
        DISPLAY,
        &[
            ("hdr", f64::from(u8::from(hdr))),
            ("accurate", f64::from(u8::from(accurate))),
        ],
    );
    true
}

pub fn save_frame(limit: u32) -> bool {
    let Some(defaults) = open() else { return false };
    defaults.setDouble_forKey(f64::from(limit), &NSString::from_str(FRAME));
    true
}

fn write_picture(defaults: &NSUserDefaults, value: Picture) {
    write_group(
        defaults,
        PICTURE,
        &[
            ("sharpen", f64::from(value.sharpen)),
            ("exposure", f64::from(value.exposure)),
            ("contrast", f64::from(value.contrast)),
            ("saturation", f64::from(value.saturation)),
            ("temperature", f64::from(value.temperature)),
            ("bloom", f64::from(value.bloom)),
            ("bloomThreshold", f64::from(value.bloom_threshold)),
            ("bloomRadius", f64::from(value.bloom_radius)),
            ("fxaa", f64::from(u8::from(value.fxaa))),
        ],
    );
}

fn write_group(defaults: &NSUserDefaults, group: &str, values: &[(&str, f64)]) {
    let keys: Vec<_> = values
        .iter()
        .map(|(key, _)| NSString::from_str(key))
        .collect();
    let numbers: Vec<_> = values
        .iter()
        .map(|(_, value)| NSNumber::new_f64(*value))
        .collect();
    let keys: Vec<_> = keys.iter().map(|key| &**key).collect();
    let numbers: Vec<_> = numbers.iter().map(|value| &**value).collect();
    let dictionary = NSDictionary::from_slices(&keys, &numbers);
    // SAFETY: immutable NSString keys and finite NSNumber values form a valid property list.
    // One dictionary replacement prevents a partially updated group from being read.
    unsafe { defaults.setObject_forKey(Some(&dictionary), &NSString::from_str(group)) };
}

fn number(value: Option<Retained<AnyObject>>) -> Option<f64> {
    let value = value?;
    let Some(number) = value.downcast_ref::<NSNumber>() else {
        log::warn!(target: "mtld3d::preferences", "Ignoring a nonnumeric graphics preference.");
        return None;
    };
    let number = number.doubleValue();
    if !number.is_finite() {
        log::warn!(target: "mtld3d::preferences", "Ignoring a nonfinite graphics preference.");
        return None;
    }
    Some(number)
}

fn group_number(dictionary: Option<&NSDictionary<NSString, AnyObject>>, key: &str) -> Option<f64> {
    number(dictionary?.objectForKey(&NSString::from_str(key)))
}

fn flag(value: Option<f64>) -> Option<bool> {
    match value? {
        0.0 => Some(false),
        1.0 => Some(true),
        _ => {
            log::warn!(target: "mtld3d::preferences", "Ignoring an invalid graphics toggle.");
            None
        }
    }
}

fn read_picture(defaults: &NSUserDefaults) -> Picture {
    let dictionary = defaults.dictionaryForKey(&NSString::from_str(PICTURE));
    let neutral = Picture::neutral();
    let scalar = |key, fallback| {
        group_number(dictionary.as_deref(), key)
            .filter(|value| value.abs() <= f64::from(f32::MAX))
            .map_or(fallback, macdrv::bounded_cast::f64_to_f32)
    };
    Picture {
        sharpen: scalar("sharpen", neutral.sharpen),
        exposure: scalar("exposure", neutral.exposure),
        contrast: scalar("contrast", neutral.contrast),
        saturation: scalar("saturation", neutral.saturation),
        temperature: scalar("temperature", neutral.temperature),
        bloom: scalar("bloom", neutral.bloom),
        bloom_threshold: scalar("bloomThreshold", neutral.bloom_threshold),
        bloom_radius: scalar("bloomRadius", neutral.bloom_radius),
        fxaa: flag(group_number(dictionary.as_deref(), "fxaa")).unwrap_or(neutral.fxaa),
    }
}

fn read_display(defaults: &NSUserDefaults) -> (Option<bool>, Option<bool>) {
    let dictionary = defaults.dictionaryForKey(&NSString::from_str(DISPLAY));
    (
        flag(group_number(dictionary.as_deref(), "hdr")),
        flag(group_number(dictionary.as_deref(), "accurate")),
    )
}

fn read_frame(defaults: &NSUserDefaults) -> Option<u32> {
    let value = number(defaults.objectForKey(&NSString::from_str(FRAME)))?;
    if !(0.0..=1000.0).contains(&value) || value.fract() != 0.0 {
        log::warn!(target: "mtld3d::preferences", "Ignoring an invalid saved frame limit.");
        return None;
    }
    u32::try_from(macdrv::bounded_cast::f64_to_u64_saturating(value)).ok()
}

#[cfg(test)]
mod tests;

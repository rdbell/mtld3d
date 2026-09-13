//! Foundation preference round trips use isolated domains, never game preferences.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT_DOMAIN: AtomicU64 = AtomicU64::new(0);

struct Domain {
    name: Retained<NSString>,
    defaults: Retained<NSUserDefaults>,
}

impl Domain {
    fn new() -> Self {
        let name = NSString::from_str(&format!(
            "org.batesai.ffxi.graphics.tests.{}.{}",
            std::process::id(),
            NEXT_DOMAIN.fetch_add(1, Ordering::Relaxed)
        ));
        let defaults =
            NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&name)).unwrap();
        defaults.removePersistentDomainForName(&name);
        Self { name, defaults }
    }

    fn reopen(&self) -> Retained<NSUserDefaults> {
        NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&self.name)).unwrap()
    }
}

impl Drop for Domain {
    fn drop(&mut self) {
        self.defaults.removePersistentDomainForName(&self.name);
    }
}

#[test]
fn fresh_domain_preserves_unset_overrides_and_neutral_picture() {
    let domain = Domain::new();
    assert_eq!(read_frame(&domain.defaults), None);
    assert_eq!(read_display(&domain.defaults), (None, None));
    let picture = read_picture(&domain.defaults);
    assert_eq!(picture.contrast, 1.0);
    assert_eq!(picture.saturation, 1.0);
    assert_eq!(picture.bloom_radius, 3.0);
    assert_eq!(picture.sharpen, 0.0);
    assert!(!picture.fxaa);
}

#[test]
fn all_picture_values_round_trip_through_fresh_defaults_instance() {
    let domain = Domain::new();
    let value = Picture {
        sharpen: 0.25,
        exposure: -0.5,
        contrast: 1.25,
        saturation: 0.75,
        temperature: -0.25,
        bloom: 0.5,
        bloom_threshold: 0.25,
        bloom_radius: 6.0,
        fxaa: true,
    };
    write_picture(&domain.defaults, value);
    let restored = read_picture(&domain.reopen());
    assert_eq!(restored.sharpen, value.sharpen);
    assert_eq!(restored.exposure, value.exposure);
    assert_eq!(restored.contrast, value.contrast);
    assert_eq!(restored.saturation, value.saturation);
    assert_eq!(restored.temperature, value.temperature);
    assert_eq!(restored.bloom, value.bloom);
    assert_eq!(restored.bloom_threshold, value.bloom_threshold);
    assert_eq!(restored.bloom_radius, value.bloom_radius);
    assert_eq!(restored.fxaa, value.fxaa);
}

#[test]
fn explicit_zero_false_and_reset_survive_without_erasing_other_groups() {
    let domain = Domain::new();
    domain
        .defaults
        .setDouble_forKey(0.0, &NSString::from_str(FRAME));
    write_group(
        &domain.defaults,
        DISPLAY,
        &[("hdr", 0.0), ("accurate", 1.0)],
    );
    let mut adjusted = Picture::neutral();
    adjusted.sharpen = 0.8;
    adjusted.fxaa = true;
    write_picture(&domain.defaults, adjusted);
    write_picture(&domain.defaults, Picture::neutral());
    let reopened = domain.reopen();
    assert_eq!(read_frame(&reopened), Some(0));
    assert_eq!(read_display(&reopened), (Some(false), Some(true)));
    assert_eq!(read_picture(&reopened).sharpen, 0.0);
    assert!(!read_picture(&reopened).fxaa);
    write_group(
        &domain.defaults,
        DISPLAY,
        &[("hdr", 1.0), ("accurate", 0.0)],
    );
    assert_eq!(read_display(&domain.reopen()), (Some(true), Some(false)));
    assert_eq!(read_frame(&domain.reopen()), Some(0));
}

#[test]
fn malformed_preferences_do_not_become_accidental_overrides() {
    let domain = Domain::new();
    let key = NSString::from_str(FRAME);
    for invalid in [-1.0, 1001.0, 59.5] {
        domain.defaults.setDouble_forKey(invalid, &key);
        assert_eq!(read_frame(&domain.defaults), None);
    }
    for valid in [0_u32, 60, 120, 1000] {
        domain.defaults.setDouble_forKey(f64::from(valid), &key);
        assert_eq!(read_frame(&domain.defaults), Some(valid));
    }
    // SAFETY: strings are valid property-list objects, although invalid for this preference.
    unsafe {
        domain
            .defaults
            .setObject_forKey(Some(&NSString::from_str("not a frame limit")), &key)
    };
    assert_eq!(read_frame(&domain.defaults), None);
    write_group(&domain.defaults, DISPLAY, &[("hdr", 7.0)]);
    assert_eq!(read_display(&domain.defaults), (None, None));
    assert_eq!(number(Some(NSNumber::new_f64(f64::NAN).into())), None);
}

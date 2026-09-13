//! Native controls for session picture adjustments.

use objc2::{MainThreadOnly, rc::Retained, runtime::Sel};
use objc2_app_kit::{
    NSButton, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSSlider, NSTextField,
    NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::SettingsDelegate;
use crate::metal::{
    macdrv,
    picture::{self, Picture},
};

const RESET_TAG: isize = 1;
const DISPLAY_TAG: isize = 2;

pub struct PictureControls {
    rows: Vec<SliderRow>,
    fxaa: Retained<NSButton>,
    hdr: Retained<NSButton>,
    accurate: Retained<NSButton>,
    reset: Retained<NSButton>,
}


impl Drop for PictureControls {
    fn drop(&mut self) {
        for row in &self.rows {
            // SAFETY: clearing a target is valid, and happens before the retained delegate drops.
            unsafe { row.slider.setTarget(None) };
        }
        for button in [&self.fxaa, &self.hdr, &self.accurate, &self.reset] {
            // SAFETY: the controls belong to this main-thread panel.
            unsafe { button.setTarget(None) };
        }
    }
}

impl PictureControls {
    pub fn new(content: &NSView, delegate: &SettingsDelegate) -> Self {
        let mtm = content.mtm();
        let value = picture::snapshot();
        let mut rows = Vec::new();
        let mut y = 508.0;
        for (label, low, high, current) in [
            ("Adaptive sharpening", 0.0, 1.0, value.sharpen),
            ("Exposure (stops)", -2.0, 2.0, value.exposure),
            ("Contrast", 0.5, 1.5, value.contrast),
            ("Saturation", 0.0, 2.0, value.saturation),
            ("Temperature (cool / warm)", -1.0, 1.0, value.temperature),
            ("Bloom strength", 0.0, 1.0, value.bloom),
            ("Bloom threshold", 0.0, 1.0, value.bloom_threshold),
            ("Bloom radius", 1.0, 8.0, value.bloom_radius),
        ] {
            let title = NSTextField::labelWithString(&NSString::from_str(label), mtm);
            title.setFrame(rect(24.0, y, 180.0, 24.0));
            content.addSubview(&title);
            // SAFETY: action() names the typed pictureChanged: callback on the retained delegate.
            let slider = unsafe { NSSlider::sliderWithValue_minValue_maxValue_target_action(
                f64::from(current), low, high, Some(delegate), Some(action()), mtm,
            ) };
            slider.setFrame(rect(208.0, y, 282.0, 24.0));
            slider.setContinuous(true);
            slider.setToolTip(Some(&NSString::from_str(label)));
            content.addSubview(&slider);
            let display = NSTextField::labelWithString(&NSString::from_str(&format!("{current:.2}")), mtm);
            display.setFrame(rect(508.0, y, 80.0, 24.0));
            content.addSubview(&display);
            rows.push(SliderRow { slider, value: display });
            y -= 32.0;
        }
        let (hdr_requested, accurate_requested) = macdrv::session_display();
        let fxaa = checkbox(content, delegate, "FXAA anti-aliasing (can soften text)", value.fxaa, 0, 248.0);
        let hdr = checkbox(content, delegate, "HDR output (requires display headroom)", hdr_requested, DISPLAY_TAG, 216.0);
        let accurate = checkbox(content, delegate, "Accurate sRGB colors (use the display color profile)", accurate_requested, DISPLAY_TAG, 184.0);
        // SAFETY: the target is retained by Settings and implements the paired typed callback.
        let reset = unsafe { NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Reset effects"), Some(delegate), Some(action()), mtm,
        ) };
        reset.setTag(RESET_TAG);
        reset.setFrame(rect(24.0, 137.0, 150.0, 30.0));
        content.addSubview(&reset);
        let note = NSTextField::labelWithString(&NSString::from_str(
            "Effects also affect game text, menus and nameplates.\nSharpening and bloom are off at 0. Neutral contrast/saturation: 1.\nAccurate colors preserves sRGB intent; off uses the existing vivid mode.\nAll changes last for this game session only."
        ), mtm);
        note.setFrame(rect(24.0, 51.0, 576.0, 78.0));
        content.addSubview(&note);
        Self { rows, fxaa, hdr, accurate, reset }
    }

    pub fn changed(&self, sender: &NSControl) -> &'static str {
        if sender.tag() == RESET_TAG {
            self.set_values(Picture::neutral());
        }
        if sender.tag() == DISPLAY_TAG {
            macdrv::set_session_display(sender.mtm(), checked(&self.hdr), checked(&self.accurate));
            return macdrv::session_display_status();
        }
        let values: Vec<_> = self.rows.iter().map(|row| {
            // Bounds come from the slider constructor and are clamped again by picture::update.
            let value = macdrv::bounded_cast::f64_to_f32(row.slider.doubleValue());
            row.value.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            value
        }).collect();
        picture::update(Picture {
            sharpen: values[0], exposure: values[1], contrast: values[2], saturation: values[3],
            temperature: values[4], bloom: values[5], bloom_threshold: values[6], bloom_radius: values[7],
            fxaa: checked(&self.fxaa),
        });
        "Picture settings applied for this session."
    }

    fn set_values(&self, value: Picture) {
        for (row, number) in self.rows.iter().zip([
            value.sharpen, value.exposure, value.contrast, value.saturation,
            value.temperature, value.bloom, value.bloom_threshold, value.bloom_radius,
        ]) {
            row.slider.setDoubleValue(f64::from(number));
        }
        self.fxaa.setState(state(value.fxaa));
    }
}

struct SliderRow {
    slider: Retained<NSSlider>,
    value: Retained<NSTextField>,
}


fn checkbox(content: &NSView, delegate: &SettingsDelegate, title: &str, enabled: bool, tag: isize, y: f64) -> Retained<NSButton> {
    // SAFETY: action() and the retained delegate form the typed callback pair documented below.
    let button = unsafe { NSButton::checkboxWithTitle_target_action(
        &NSString::from_str(title), Some(delegate), Some(action()), content.mtm(),
    ) };
    button.setState(state(enabled));
    button.setTag(tag);
    button.setFrame(rect(24.0, y, 576.0, 26.0));
    content.addSubview(&button);
    button
}

fn checked(button: &NSButton) -> bool { button.state() == NSControlStateValueOn }
fn state(enabled: bool) -> isize { if enabled { NSControlStateValueOn } else { NSControlStateValueOff } }
fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

/// AppKit's target/action API needs a selector token, not a Rust method pointer.
/// The paired method is defined on SettingsDelegate with a typed NSControl sender.
fn action() -> Sel {
    objc2::sel!(pictureChanged:)
}

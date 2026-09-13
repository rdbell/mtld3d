//! Session presentation controls in a native, nonmodal settings window.

use core::{ptr::NonNull, sync::atomic::Ordering};

use block2::RcBlock;
use objc2::{
    MainThreadOnly, define_class, extern_methods,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSBackingStoreType, NSControl, NSControlTextEditingDelegate, NSEvent, NSEventMask,
    NSEventModifierFlags, NSPanel, NSTextField, NSTextFieldDelegate, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::{
    super::{PRESENT_PACING_BITS, set_display_sync_enabled, unpack_pacing},
    FRAME_LIMIT, current, frame_limit,
};

mod picture_controls;

pub struct Settings {
    picture: picture_controls::PictureControls,
    panel: Retained<NSPanel>,
    value: Retained<NSTextField>,
    status: Retained<NSTextField>,
    _delegate: Retained<SettingsDelegate>,
}

impl Drop for Settings {
    fn drop(&mut self) {
        // SAFETY: the field and its main-thread delegate are retained through this call.
        unsafe { self.value.setDelegate(None) };
        self.panel.setDelegate(None);
        self.panel.close();
    }
}

define_class!(
    // SAFETY: NSObject has no additional subclass requirements; this class has no ivars.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct SettingsDelegate;

    // SAFETY: NSObjectProtocol has no extra invariants.
    unsafe impl NSObjectProtocol for SettingsDelegate {}

    // SAFETY: AppKit calls the typed editing callback on its main thread.
    unsafe impl NSControlTextEditingDelegate for SettingsDelegate {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn edited(&self, _notification: &NSNotification) {
            let Some(host) = current() else { return };
            let settings = host.settings.borrow();
            let Some(settings) = settings.as_ref() else {
                return;
            };
            let text = settings.value.stringValue().to_string();
            let Ok(value) = text.trim().parse::<u32>() else {
                settings
                    .status
                    .setStringValue(&NSString::from_str("Enter a whole number from 0 to 1000."));
                return;
            };
            if value > 1000 {
                settings
                    .status
                    .setStringValue(&NSString::from_str("Enter a whole number from 0 to 1000."));
                return;
            }
            FRAME_LIMIT.store(value, Ordering::Relaxed);
            let pacing = unpack_pacing(PRESENT_PACING_BITS.load(Ordering::Relaxed));
            set_display_sync_enabled(mtld3d_shared::MetalHandle::NULL, &pacing);
            settings
                .status
                .setStringValue(&NSString::from_str(if value == 0 {
                    "Frame limit removed for this session."
                } else {
                    "Frame limit applied for this session."
                }));
            log::info!(target: crate::LOG_TARGET, "native host: session frame limit set to {value}");
        }
    }

    impl SettingsDelegate {
        #[unsafe(method(pictureChanged:))]
        fn picture_changed(&self, sender: &NSControl) {
            let Some(host) = current() else { return };
            let settings = host.settings.borrow();
            let Some(settings) = settings.as_ref() else { return };
            let status = settings.picture.changed(sender);
            settings.status.setStringValue(&NSString::from_str(&status));
        }
    }

    // SAFETY: NSTextFieldDelegate adds no required callbacks.
    unsafe impl NSTextFieldDelegate for SettingsDelegate {}

    // SAFETY: AppKit delivers this notification on the main thread.
    unsafe impl NSWindowDelegate for SettingsDelegate {
        #[unsafe(method(windowWillClose:))]
        fn closed(&self, _notification: &NSNotification) {
            let Some(host) = current() else { return };
            let view = host.view;
            super::super::run_on_main_thread_async(move || {
                let Some(host) = current().filter(|host| host.view == view) else {
                    return;
                };
                let app = objc2_app_kit::NSApplication::sharedApplication(host.window.mtm());
                if !app.isActive() {
                    return;
                }
                if let Some(settings) = host.settings.borrow().as_ref()
                    && settings.panel.isVisible()
                {
                    return;
                }
                if app
                    .keyWindow()
                    .is_some_and(|key| key != host.child && key != host.window)
                {
                    return;
                }
                host.child.makeKeyWindow();
                if let Some(content) = host.child.contentView() {
                    host.child.makeFirstResponder(Some(&content));
                }
            });
        }
    }
);

impl SettingsDelegate {
    extern_methods!(
        #[unsafe(method(new))]
        fn new(mtm: MainThreadMarker) -> Retained<Self>;
    );
}

/// Keep the shortcut local to this process and remove it at device teardown.
pub fn install_shortcut() -> Option<Retained<AnyObject>> {
    let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit lends this live event only for the monitor callback.
        let event_ref = unsafe { event.as_ref() };
        let modifiers = event_ref
            .modifierFlags()
            .intersection(NSEventModifierFlags::DeviceIndependentFlagsMask);
        if let Some(host) = current()
            && event_ref
                .window(host.window.mtm())
                .is_some_and(|window| window == host.child || window == host.window)
            && let Some(key) = event_ref.charactersIgnoringModifiers()
        {
            let key = key.to_string();
            if modifiers == NSEventModifierFlags::Command {
                match key.as_str() {
                    "," => show(),
                    "w" => {
                        if let Some(delegate) = host.child.delegate() {
                            delegate.windowShouldClose(&host.child);
                        }
                    }
                    "m" => host.window.miniaturize(None),
                    _ => return event.as_ptr(),
                }
                return core::ptr::null_mut();
            }
            if modifiers == NSEventModifierFlags::Command | NSEventModifierFlags::Control
                && key == "f"
            {
                host.window.toggleFullScreen(None);
                return core::ptr::null_mut();
            }
        }
        event.as_ptr()
    });
    // SAFETY: AppKit copies the block and the returned monitor is retained until detach.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    if monitor.is_none() {
        log::warn!(target: crate::LOG_TARGET, "native host: settings keyboard shortcut is unavailable");
    }
    monitor
}

/// Remove the owned event monitor on `AppKit`'s main thread.
pub fn remove_shortcut(monitor: &AnyObject) {
    // SAFETY: this token came from addLocalMonitor and is removed exactly once at detach.
    unsafe { NSEvent::removeMonitor(monitor) };
}

fn show() {
    let Some(host) = current() else { return };
    let panel = {
        let mut slot = host.settings.borrow_mut();
        let settings = slot.get_or_insert_with(|| Settings::new(host.window.mtm()));
        if crate::metal::interpolation::settings().0 {
            settings.status.setStringValue(&NSString::from_str(&crate::metal::interpolation::status()));
        }
        settings.panel.clone()
    };
    panel.makeKeyAndOrderFront(None);
}

impl Settings {
    fn new(mtm: MainThreadMarker) -> Self {
        // SAFETY: Rust retains the panel; release-on-close is disabled immediately below.
        let panel = {
            NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(620.0, 760.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the Rust retain owns the panel even after a user closes it.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&NSString::from_str("Graphics Settings"));
        let content = panel.contentView().expect("a new panel has a content view");
        let label = NSTextField::labelWithString(&NSString::from_str("Frame limit"), mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(24.0, 706.0),
            NSSize::new(150.0, 24.0),
        ));
        let pacing = unpack_pacing(PRESENT_PACING_BITS.load(Ordering::Relaxed));
        let value = NSTextField::textFieldWithString(
            &NSString::from_str(&frame_limit(pacing.max_fps).to_string()),
            mtm,
        );
        value.setFrame(NSRect::new(
            NSPoint::new(208.0, 704.0),
            NSSize::new(100.0, 26.0),
        ));
        let note = NSTextField::labelWithString(
            &NSString::from_str(
                "0 means no limit. Press Return to apply.\nThe game may have its own lower frame limit.",
            ),
            mtm,
        );
        note.setFrame(NSRect::new(
            NSPoint::new(24.0, 654.0),
            NSSize::new(576.0, 48.0),
        ));
        let status = NSTextField::labelWithString(
            &NSString::from_str("Changes apply to the current session."),
            mtm,
        );
        status.setFrame(NSRect::new(
            NSPoint::new(24.0, 8.0),
            NSSize::new(576.0, 40.0),
        ));
        for field in [&label, &value, &note, &status] {
            // SAFETY: all views live on this AppKit thread and the content view retains them.
            content.addSubview(field);
        }
        let delegate = SettingsDelegate::new(mtm);
        let picture = picture_controls::PictureControls::new(&content, &delegate);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // SAFETY: the typed delegate is retained alongside the field until Settings is dropped.
        unsafe { value.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        if let Some(host) = current() {
            let frame = host.window.frame();
            panel.setFrameOrigin(NSPoint::new(
                frame.origin.x + (frame.size.width - 620.0) / 2.0,
                frame.origin.y + (frame.size.height - 788.0) / 2.0,
            ));
        } else {
            panel.center();
        }
        panel.setCollectionBehavior(
            objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary
                | objc2_app_kit::NSWindowCollectionBehavior::MoveToActiveSpace,
        );
        Self {
            picture,
            panel,
            value,
            status,
            _delegate: delegate,
        }
    }
}

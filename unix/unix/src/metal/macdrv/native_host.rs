//! Native window ownership with Wine retaining game input and Metal presentation.
//!
//! All state and callbacks live on the `AppKit` main thread. The child remains a
//! real Wine window so its event queue, IME and cursor machinery stay attached.

use core::{
    ffi::c_void,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use log::{info, warn};
use objc2::{
    MainThreadOnly, define_class, extern_methods, msg_send,
    rc::{Allocated, Retained},
    runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowDelegate,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRect, NSSize, NSString,
};

use super::MACDRV_LIB;
use crate::LOG_TARGET;

mod preferences;
mod settings;

static HOST_VIEW: AtomicUsize = AtomicUsize::new(0);

static FRAME_LIMIT: AtomicU32 = AtomicU32::new(u32::MAX);

thread_local! {
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

struct Host {
    window: Retained<NSWindow>,
    child: Retained<NSWindow>,
    _delegate: Retained<HostDelegate>,
    view: usize,
    bind_window: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    old_style: NSWindowStyleMask,
    old_collection: NSWindowCollectionBehavior,
    synchronizing: Cell<bool>,
    settings: RefCell<Option<settings::Settings>>,
    monitor: Option<Retained<objc2::runtime::AnyObject>>,
    close_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

define_class!(
    // SAFETY: NSWindow subclass adds no storage; all host state is main-thread confined.
    #[unsafe(super = NSWindow)]
    #[thread_kind = MainThreadOnly]
    struct NativeHostWindow;

    // SAFETY: NSObjectProtocol has no additional implementation requirements.
    unsafe impl NSObjectProtocol for NativeHostWindow {}

    impl NativeHostWindow {
        #[unsafe(method(miniaturize:))]
        fn request_minimize(&self, sender: Option<&objc2::runtime::AnyObject>) {
            if let Some(host) = current() {
                host.child.miniaturize(sender);
            }
        }

        #[unsafe(method(nativeHostMiniaturizeFromWine:))]
        fn minimize_from_wine(&self, sender: Option<&objc2::runtime::AnyObject>) {
            // SAFETY: invoke NSWindow's implementation after Win32 accepted minimization.
            // The typed method dispatches back to our override; objc2 has no typed super binding.
            unsafe {
                let _: () = msg_send![super(self), miniaturize: sender];
            }
        }
    }
);

impl NativeHostWindow {
    extern_methods!(
        // SAFETY: inherited NSWindow initializer; this subclass adds no ivars or Drop state.
        #[unsafe(method(initWithContentRect:styleMask:backing:defer:))]
        unsafe fn init_with_content_rect(
            this: Allocated<Self>,
            content_rect: NSRect,
            style: NSWindowStyleMask,
            backing: NSBackingStoreType,
            deferred: bool,
        ) -> Retained<Self>;
    );
}

define_class!(
    // SAFETY: NSObject has no additional subclass invariants; the delegate has no ivars.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct HostDelegate;

    // SAFETY: NSObjectProtocol has no additional implementation requirements.
    unsafe impl NSObjectProtocol for HostDelegate {}

    // SAFETY: Each callback follows the NSWindowDelegate signature and runs on the main thread.
    unsafe impl NSWindowDelegate for HostDelegate {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            if let Some(host) = current()
                && let Some(delegate) = host.child.delegate()
            {
                delegate.windowShouldClose(&host.child);
            }
            // Wine must deliver WM_CLOSE and allow the game to confirm or cancel.
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn resized(&self, _notification: &NSNotification) {
            if let Some(host) = current() {
                host.sync_child();
            }
        }

        #[unsafe(method(windowDidMove:))]
        fn moved(&self, _notification: &NSNotification) {
            if let Some(host) = current() {
                host.sync_child();
            }
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn became_key(&self, _notification: &NSNotification) {
            // AppKit finishes selecting its key window after this notification returns.
            // A synchronous transfer can be overwritten by that transition.
            super::run_on_main_thread_async(|| {
                if let Some(host) = current()
                    && host.window.isKeyWindow()
                {
                    host.child.makeKeyWindow();
                    log::debug!(target: "mtld3d::native_host", "focus handoff: host_key={} game_key={} game_eligible={}",
                        host.window.isKeyWindow(), host.child.isKeyWindow(), host.child.canBecomeKeyWindow());
                }
            });
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn minimized(&self, notification: &NSNotification) {
            if let Some(host) = current()
                && let Some(delegate) = host.child.delegate()
            {
                delegate.windowDidMiniaturize(notification);
            }
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn restored(&self, notification: &NSNotification) {
            if let Some(host) = current() {
                if let Some(delegate) = host.child.delegate() {
                    delegate.windowDidDeminiaturize(notification);
                }
                host.sync_child();
                host.focus_after_transition();
            }
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn entered_fullscreen(&self, _notification: &NSNotification) {
            if let Some(host) = current() {
                host.sync_child();
                host.focus_after_transition();
            }
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn exited_fullscreen(&self, _notification: &NSNotification) {
            if let Some(host) = current() {
                host.sync_child();
                host.focus_after_transition();
            }
        }
    }
);

impl HostDelegate {
    extern_methods!(
        #[unsafe(method(new))]
        fn new(mtm: MainThreadMarker) -> Retained<Self>;
    );
}

/// Attach one windowed presentation surface; incompatible windows retain Wine ownership.
pub fn attach(view: &NSView) {
    let mtm = view.mtm();
    // SAFETY: the versioned Wine export has this ABI and the library lives for the process.
    let set_host = unsafe {
        MACDRV_LIB.get::<unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32>(
            b"macdrv_set_native_host_v1\0",
        )
    };
    let Ok(set_host) = set_host else {
        warn!(target: LOG_TARGET, "native host: Wine lacks the native-host contract; keeping Wine window");
        return;
    };
    let set_host = *set_host;
    if owns(core::ptr::from_ref(view) as usize) {
        return;
    }
    if current().is_some() {
        warn!(target: LOG_TARGET, "native host: another surface is already hosted; keeping Wine window");
        return;
    }
    let Some(child) = view.window() else {
        warn!(target: LOG_TARGET, "native host: surface has no window; keeping Wine window");
        return;
    };
    if child.parentWindow().is_some() || child.styleMask().contains(NSWindowStyleMask::FullScreen) {
        warn!(target: LOG_TARGET, "native host: child or fullscreen window is unsupported; keeping Wine window");
        return;
    }
    preferences::restore(mtm);
    let content = child.contentRectForFrameRect(child.frame());
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // SAFETY: creates a retained main-thread window; release-on-close is disabled immediately below.
    let window = unsafe {
        NativeHostWindow::init_with_content_rect(
            NativeHostWindow::alloc(mtm),
            content,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let window = Retained::into_super(window);
    // SAFETY: Rust owns the retain and releases it at detach, not at Cocoa close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Final Fantasy XI"));
    window.setBackgroundColor(Some(&NSColor::blackColor()));
    window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
    // SAFETY: positive finite native point dimensions on the main thread.
    window.setContentMinSize(NSSize::new(640.0, 360.0));
    let delegate = HostDelegate::new(mtm);
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let old_style = child.styleMask();
    let old_collection = child.collectionBehavior();
    // SAFETY: view and host are live AppKit objects on the main thread; detach clears the binding.
    if unsafe {
        set_host(
            Retained::as_ptr(&child).cast_mut().cast(),
            Retained::as_ptr(&window).cast_mut().cast(),
        )
    } == 0
    {
        window.setDelegate(None);
        window.close();
        warn!(target: LOG_TARGET, "native host: Wine declined hosting; keeping Wine window");
        return;
    }
    child.setStyleMask(NSWindowStyleMask::Borderless);
    if let Some(content) = child.contentView() {
        child.makeFirstResponder(Some(&content));
    }
    child.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenAuxiliary);
    // SAFETY: both windows are live, main-thread objects, with no existing parent relationship.
    unsafe { window.addChildWindow_ordered(&child, NSWindowOrderingMode::Above) };
    let address = core::ptr::from_ref(view) as usize;
    let block = block2::RcBlock::new(move |_notification: core::ptr::NonNull<NSNotification>| {
        detach(address);
    });
    // SAFETY: the retained observer is removed at detach, and the block captures only an address.
    let close_observer = unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(objc2_app_kit::NSWindowWillCloseNotification),
                Some(&child),
                None,
                &block,
            )
    };
    let host = Rc::new(Host {
        window,
        child,
        _delegate: delegate,
        view: core::ptr::from_ref(view) as usize,
        bind_window: set_host,
        old_style,
        old_collection,
        synchronizing: Cell::new(false),
        settings: RefCell::new(None),
        monitor: settings::install_shortcut(),
        close_observer,
    });
    HOST.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&host)));
    HOST_VIEW.store(address, Ordering::Release);
    host.sync_child();
    host.window.makeKeyAndOrderFront(None);
    host.child.makeKeyAndOrderFront(None);
    info!(target: LOG_TARGET, "native host: attached window={} child={} with direct Metal presentation",
        host.window.windowNumber(), host.child.windowNumber());
}

/// Restore Wine ownership before its Metal view is released.
pub fn detach(view: usize) {
    let host = HOST.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|host| host.view == view) {
            slot.take()
        } else {
            None
        }
    });
    let Some(host) = host else { return };
    HOST_VIEW.store(0, Ordering::Release);
    // SAFETY: the token belongs to the default notification center and is still retained.
    unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter()
            .removeObserver((*host.close_observer).as_ref());
    };
    if let Some(monitor) = &host.monitor {
        settings::remove_shortcut(monitor);
    }
    host.settings.borrow_mut().take();
    host.window.setDelegate(None);
    // SAFETY: Host retains this exact Wine window even after Wine removes the view.
    let cleared = unsafe {
        (host.bind_window)(
            Retained::as_ptr(&host.child).cast_mut().cast(),
            core::ptr::null_mut(),
        )
    };
    assert_ne!(
        cleared, 0,
        "Wine must clear the retained child's host association"
    );
    // SAFETY: the child belongs to this live host and all work runs on the main thread.
    host.window.removeChildWindow(&host.child);
    host.child.setStyleMask(host.old_style);
    host.child.setCollectionBehavior(host.old_collection);
    host.window.orderOut(None);
    host.window.close();
    info!(target: LOG_TARGET, "native host: detached and restored Wine window ownership");
}

fn current() -> Option<Rc<Host>> {
    HOST.with(|slot| slot.borrow().as_ref().map(Rc::clone))
}

impl Host {
    fn focus_after_transition(&self) {
        let view = self.view;
        super::run_on_main_thread_async(move || {
            if let Some(host) = current().filter(|host| host.view == view) {
                let app = objc2_app_kit::NSApplication::sharedApplication(host.window.mtm());
                if !app.isActive()
                    || app
                        .keyWindow()
                        .is_some_and(|key| key != host.window && key != host.child)
                {
                    return;
                }
                host.child.makeKeyWindow();
                if let Some(content) = host.child.contentView() {
                    host.child.makeFirstResponder(Some(&content));
                }
                log::debug!(target: "mtld3d::native_host", "transition focus: game_key={}", host.child.isKeyWindow());
            }
        });
    }

    fn sync_child(&self) {
        if self.synchronizing.replace(true) {
            return;
        }
        let frame = self.window.contentRectForFrameRect(self.window.frame());
        self.child.setFrame_display(frame, true);
        self.synchronizing.set(false);
    }
}

/// Refit the hosted game after a display configuration change. **Main thread only.**
///
/// Wine answers a display change on the Win32 side, which can shrink the game's Win32 window
/// to fit the displays of the moment. Its request to resize the Cocoa window is ignored while
/// the host owns it, so the window keeps its size while Wine's views follow the smaller Win32
/// rectangle: the game draws into part of the window until a move or resize makes Wine read
/// the frame again. The Win32 side settles over a few seconds, so the check repeats.
pub fn after_display_change() {
    resend_frame_if_stale();
    std::thread::spawn(|| {
        for delay_ms in [250, 750, 1000, 2000, 4000] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            super::run_on_main_thread_async(resend_frame_if_stale);
        }
    });
}

/// Largest gap, in points, between the game view and the window it hosts in normal play.
///
/// Wine's Win32 frame leaves a few points (4 measured) between the view and the content area.
const VIEW_INSET_SLACK: f64 = 16.0;

/// Post the child's frame to Wine again when the game view no longer fills the window.
fn resend_frame_if_stale() {
    let Some(host) = current() else {
        return;
    };
    let Some(content) = host.child.contentView() else {
        return;
    };
    // SAFETY: the hosted view outlives the host; detach runs before Wine releases it.
    let view = unsafe { &*(host.view as *const NSView) };
    let drawn = view.convertRect_toView(view.bounds(), None);
    let window = content.frame();
    if window.size.width - drawn.size.width <= VIEW_INSET_SLACK
        && window.size.height - drawn.size.height <= VIEW_INSET_SLACK
    {
        return;
    }
    info!(target: LOG_TARGET,
        "native host: game view {:.0}x{:.0} in a {:.0}x{:.0} window after a display change; resending the frame to Wine",
        drawn.size.width, drawn.size.height, window.size.width, window.size.height);
    // Wine's window delegate answers a resize by posting the current frame to the Win32 side,
    // the same path a drag takes.
    // SAFETY: AppKit-exported notification-name constant, valid for the process lifetime.
    let name = unsafe { objc2_app_kit::NSWindowDidResizeNotification };
    // SAFETY: posting on the main thread to observers of a live window.
    unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter()
            .postNotificationName_object(name, Some(&host.child));
    }
}

/// Resolve the session override without touching `AppKit` on the submit thread.
pub fn frame_limit(configured: u32) -> u32 {
    match FRAME_LIMIT.load(Ordering::Relaxed) {
        u32::MAX => configured,
        value => value,
    }
}

/// Cheap ownership check for teardown, without involving the main thread when disabled.
pub fn owns(view: usize) -> bool {
    view != 0 && HOST_VIEW.load(Ordering::Acquire) == view
}

/// Match a host notification to the active binding. Main thread only.
pub fn is_host_window(window: usize) -> bool {
    current().is_some_and(|host| Retained::as_ptr(&host.window) as usize == window)
}

//! Opt-in observation of scheduling classes on threads crossing the native boundary.

use std::cell::Cell;

thread_local! {
    static LAST: Cell<[(u32, i32, i32); 3]> = const {
        Cell::new([(u32::MAX, i32::MIN, i32::MIN); 3])
    };
}

/// Rendering work whose calling thread is observed.
pub enum Role {
    DeviceApi,
    Submit,
    ReadbackApi,
}

/// Record the initial class and subsequent changes for each role on this thread.
///
/// Queries the current pthread only; this never changes scheduling policy.
pub fn observe(role: &Role) {
    if !log::log_enabled!(target: "mtld3d::qos", log::Level::Debug) {
        return;
    }
    let (index, name) = match role {
        Role::DeviceApi => (0, "device-api"),
        Role::Submit => (1, "submit"),
        Role::ReadbackApi => (2, "readback-api"),
    };
    let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
    let mut relative_priority = 0;
    // SAFETY: querying the current pthread has no preconditions.
    let current = unsafe { libc::pthread_self() };
    // SAFETY: current is a live pthread; both outputs are writable scalars.
    let status = unsafe {
        libc::pthread_get_qos_class_np(current, &raw mut class, &raw mut relative_priority)
    };
    // SAFETY: current is the live calling thread.
    let thread = unsafe { libc::pthread_mach_thread_np(current) };
    let class = class as u32;
    LAST.with(|last| {
        let mut previous = last.get();
        if previous[index] == (class, relative_priority, status) {
            return;
        }
        previous[index] = (class, relative_priority, status);
        last.set(previous);
        log::debug!(target: "mtld3d::qos",
            "role={name} mach_thread={thread} class={class} relative_priority={relative_priority} status={status}");
    });
}

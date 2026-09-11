//! Keep ordinary 32-bit data pages non-executable under Wine.
//!
//! Loading a legacy DLL without `NX_COMPAT` makes Wine enable execute on every page.
//! Rosetta then routes writes to fresh renderer buffers through its exception server.
//! FFXI's Chainspell effect reproduced five roughly 1.26-second frames on every use;
//! restoring DEP removed those stalls in a matched local diagnostic. Explicitly
//! executable code pages are unaffected. This mirrors the fork's DXVK correction.

#[cfg(target_pointer_width = "32")]
pub fn enforce() {
    use core::ffi::c_void;
    use std::sync::Once;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process: *mut c_void,
            class: u32,
            data: *mut u32,
            size: u32,
            returned: *mut u32,
        ) -> i32;
        fn NtSetInformationProcess(
            process: *mut c_void,
            class: u32,
            data: *const u32,
            size: u32,
        ) -> i32;
    }

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("MTLD3D_ENFORCE_NX").as_deref() == Some(std::ffi::OsStr::new("0")) {
            return;
        }
        let process = usize::MAX as *mut c_void;
        let mut before = 0;
        // SAFETY: current-process pseudo-handle, ProcessExecuteFlags, and a writable ULONG.
        let queried = unsafe {
            NtQueryInformationProcess(process, 0x22, &raw mut before, 4, core::ptr::null_mut())
        };
        if queried != 0 {
            log::warn!(target: crate::LOG_TARGET, "NX policy query failed: {queried:#x}");
            return;
        }
        // MEM_EXECUTE_OPTION_DISABLE | MEM_EXECUTE_OPTION_PERMANENT. The permanent bit
        // keeps later legacy DLL loads from reverting the policy during gameplay.
        let flags = 0x9;
        if before & flags == flags {
            log::info!(target: crate::LOG_TARGET, "NX already enforced: {before:#x}");
            return;
        }
        // SAFETY: same current process and information class, with a readable ULONG.
        let status = unsafe { NtSetInformationProcess(process, 0x22, &raw const flags, 4) };
        let mut after = before;
        // SAFETY: current-process policy query into a writable ULONG.
        let verified = unsafe {
            NtQueryInformationProcess(process, 0x22, &raw mut after, 4, core::ptr::null_mut())
        };
        if status == 0 && verified == 0 && after & flags == flags {
            log::info!(target: crate::LOG_TARGET, "NX enforced: {before:#x} -> {after:#x}");
        } else {
            log::warn!(target: crate::LOG_TARGET,
                "NX enforcement unconfirmed: {before:#x} -> {after:#x}, set={status:#x}, query={verified:#x}");
        }
    });
}

// 64-bit Windows processes already enforce DEP and cannot change this policy.
#[cfg(target_pointer_width = "64")]
pub const fn enforce() {}

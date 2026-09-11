//! Unit tests for the typed FFI-boundary pointer wrappers.
//!
//! A local `#[repr(C)]` struct stands in for a C in-param: the tests pin that `opt` filters null
//! on `InPtr` and `OutPtr`, that reads and writes through `InPtr`, `InPtrMut`, `ValueIn`,
//! `OutPtr` and `VtableThis` land on the original storage, and that `InPtr`, `Option<InPtr>`
//! (which relies on the null-pointer niche), `OutPtr` and `VtableThis` are still exactly
//! pointer-sized, so those four cost nothing at the call seam.

use super::*;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Point {
    x: i32,
    y: i32,
}

#[test]
fn in_ptr_opt_filters_null() {
    // SAFETY: passing literal null is sound; opt filters it.
    let opt: Option<InPtr<'_, Point>> = unsafe { InPtr::opt(core::ptr::null()) };
    assert!(opt.is_none());
}

#[test]
fn in_ptr_round_trip() {
    let p = Point { x: 7, y: -3 };
    let raw: *const c_void = (&raw const p).cast();
    // SAFETY: `raw` points to a live local `Point` for the call frame.
    let wrap: InPtr<'_, Point> = unsafe { InPtr::opt(raw) }.unwrap();
    assert_eq!(*wrap, p);
}

#[test]
fn in_ptr_mut_round_trip() {
    let mut p = Point { x: 1, y: 2 };
    let raw: *mut c_void = (&raw mut p).cast();
    // SAFETY: exclusive access — local `p` not aliased.
    let mut wrap: InPtrMut<'_, Point> = unsafe { InPtrMut::opt(raw) }.unwrap();
    wrap.x = 99;
    assert_eq!(p.x, 99);
}

#[test]
fn value_in_reads_by_value() {
    let p = Point { x: 5, y: 6 };
    let raw: *const c_void = (&raw const p).cast();
    // SAFETY: `raw` points to a live local `Point`.
    let v: ValueIn<'_, Point> = unsafe { ValueIn::opt(raw) }.unwrap();
    assert_eq!(v.read(), p);
}

#[test]
fn out_ptr_writes_through_pointer() {
    let mut p = Point { x: 0, y: 0 };
    let raw: *mut Point = &raw mut p;
    // SAFETY: `raw` points to a writable local.
    let o: OutPtr<'_, Point> = unsafe { OutPtr::opt(raw) }.unwrap();
    o.write(Point { x: 11, y: 22 });
    assert_eq!(p, Point { x: 11, y: 22 });
}

#[test]
fn out_ptr_opt_filters_null() {
    // SAFETY: null is sound; opt filters it.
    let opt: Option<OutPtr<'_, Point>> = unsafe { OutPtr::opt(core::ptr::null_mut()) };
    assert!(opt.is_none());
}

#[test]
fn out_ptr_writes_unaligned_storage_without_touching_neighbors() {
    let mut storage = [u64::MAX; 3];
    let raw = storage.as_mut_ptr().wrapping_byte_add(2);
    assert!(!raw.is_aligned());
    // SAFETY: `raw` spans eight writable bytes in the local storage; alignment is not required.
    let out = unsafe { OutPtr::opt(raw) }.unwrap();
    out.write(0x1122_3344_5566_7788);
    let bytes: Vec<_> = storage.iter().flat_map(|word| word.to_ne_bytes()).collect();
    assert_eq!(&bytes[2..10], &0x1122_3344_5566_7788_u64.to_ne_bytes());
    assert!(bytes[..2].iter().all(|byte| *byte == 0xff));
    assert!(bytes[10..].iter().all(|byte| *byte == 0xff));
}

#[test]
fn vtable_this_round_trip() {
    let mut p = Point { x: 10, y: 20 };
    let raw: *mut c_void = (&raw mut p).cast();
    // SAFETY: simulating an IUnknown thunk entry with a live local.
    let wrap: VtableThis<'_, Point> = unsafe { VtableThis::new(raw) };
    assert_eq!(*wrap, Point { x: 10, y: 20 });
}

#[test]
fn types_are_zero_cost() {
    assert_eq!(
        core::mem::size_of::<InPtr<'_, Point>>(),
        core::mem::size_of::<*const Point>(),
    );
    assert_eq!(
        core::mem::size_of::<Option<InPtr<'_, Point>>>(),
        core::mem::size_of::<*const Point>(),
    );
    assert_eq!(
        core::mem::size_of::<OutPtr<'_, Point>>(),
        core::mem::size_of::<*mut Point>(),
    );
    assert_eq!(
        core::mem::size_of::<VtableThis<'_, Point>>(),
        core::mem::size_of::<*mut Point>(),
    );
}

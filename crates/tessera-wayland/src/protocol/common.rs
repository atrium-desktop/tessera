use crate::*;

// ----- shared request handlers --------------------------------------------

pub(crate) unsafe extern "C" fn res_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}
pub(crate) unsafe extern "C" fn xdg_noop_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
) {
}
pub(crate) unsafe extern "C" fn xdg_noop_menu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
    _x: i32,
    _y: i32,
) {
}

// ----- accessors for the extensions module --------------------------------

/// Construct a `tessera_primitives::Point`.
pub(crate) fn tessera_point(x: i32, y: i32) -> tessera_primitives::Point {
    tessera_primitives::Point { x, y }
}

pub(crate) fn tessera_size(w: i32, h: i32) -> tessera_primitives::Size {
    tessera_primitives::Size { w, h }
}

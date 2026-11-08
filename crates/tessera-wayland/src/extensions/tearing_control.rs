use super::*;

// ----- wp_tearing_control_v1 (ADR-0219) -----------------------------------

struct TearingControlRec {
    surface: *mut SurfaceRec,
}

static TEARING_CONTROL_MANAGER_IMPL: ffi::wp_tearing_control_manager_v1_interface_impl =
    ffi::wp_tearing_control_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_tearing_control: tearing_control_manager_get,
    };

static TEARING_CONTROL_IMPL: ffi::wp_tearing_control_v1_interface_impl =
    ffi::wp_tearing_control_v1_interface_impl {
        set_presentation_hint: tearing_control_set_presentation_hint,
        destroy: tearing_control_destroy,
    };

pub(crate) unsafe extern "C" fn tearing_control_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::checked_resource_create(
            client,
            &ffi::wp_tearing_control_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &TEARING_CONTROL_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn tearing_control_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).tearing_control.is_null() {
            ffi::wl_resource_post_error(
                mgr,
                ffi::WP_TEARING_CONTROL_MANAGER_V1_ERROR_TEARING_CONTROL_EXISTS,
                c"wl_surface already has a tearing-control object".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(mgr);
        let res =
            ffi::checked_resource_create(client, &ffi::wp_tearing_control_v1_interface, ver, id);
        if res.is_null() {
            return;
        }
        let tc_rec = Box::into_raw(Box::new(TearingControlRec { surface: rec }));
        ffi::wl_resource_set_implementation(
            res,
            &TEARING_CONTROL_IMPL as *const _ as *const c_void,
            tc_rec as *mut c_void,
            Some(tearing_control_resource_destroy),
        );
        (*rec).tearing_control = res;
    }
}

unsafe extern "C" fn tearing_control_set_presentation_hint(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    hint: u32,
) {
    unsafe {
        let tc = ffi::wl_resource_get_user_data(resource) as *mut TearingControlRec;
        if tc.is_null() || (*tc).surface.is_null() {
            return;
        }
        let surface = (*tc).surface;
        let hint_enum = match hint {
            ffi::WP_TEARING_CONTROL_V1_PRESENTATION_HINT_ASYNC => {
                tessera_primitives::PresentationHint::Async
            }
            _ => tessera_primitives::PresentationHint::Vsync,
        };
        (*surface).pending_presentation_hint = hint_enum;
    }
}

unsafe extern "C" fn tearing_control_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let tc = ffi::wl_resource_get_user_data(resource) as *mut TearingControlRec;
        if !tc.is_null() && !(*tc).surface.is_null() {
            let surface = (*tc).surface;
            (*surface).pending_presentation_hint = tessera_primitives::PresentationHint::Vsync;
            if (*surface).tearing_control == resource {
                (*surface).tearing_control = std::ptr::null_mut();
            }
            (*tc).surface = std::ptr::null_mut();
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn tearing_control_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let tc = ffi::wl_resource_get_user_data(resource) as *mut TearingControlRec;
        if tc.is_null() {
            return;
        }
        if !(*tc).surface.is_null() {
            let surface = (*tc).surface;
            if (*surface).tearing_control == resource {
                (*surface).tearing_control = std::ptr::null_mut();
            }
        }
        drop(Box::from_raw(tc));
    }
}

pub(crate) unsafe fn tearing_control_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).tearing_control.is_null() {
            return;
        }
        let tc =
            ffi::wl_resource_get_user_data((*surface).tearing_control) as *mut TearingControlRec;
        if !tc.is_null() {
            (*tc).surface = std::ptr::null_mut();
        }
        (*surface).tearing_control = std::ptr::null_mut();
    }
}

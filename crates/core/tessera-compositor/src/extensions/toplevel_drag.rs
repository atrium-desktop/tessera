use super::*;
use crate::DataSourceRec;

// ----- xdg-toplevel-drag-v1 -----------------------------------------------

pub(crate) struct ToplevelDragRec {
    pub(crate) state: *mut State,
    pub(crate) source: *mut ffi::wl_resource,
    pub(crate) attached_toplevel: *mut ffi::wl_resource,
    pub(crate) offset: (i32, i32),
    pub(crate) ended: bool,
}

static XDG_TOPLEVEL_DRAG_MANAGER_IMPL: ffi::xdg_toplevel_drag_manager_v1_interface_impl =
    ffi::xdg_toplevel_drag_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_xdg_toplevel_drag: toplevel_drag_manager_get_drag,
    };

static XDG_TOPLEVEL_DRAG_IMPL: ffi::xdg_toplevel_drag_v1_interface_impl =
    ffi::xdg_toplevel_drag_v1_interface_impl {
        destroy: toplevel_drag_destroy,
        attach: toplevel_drag_attach,
    };

pub(crate) unsafe extern "C" fn xdg_toplevel_drag_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::xdg_toplevel_drag_manager_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &XDG_TOPLEVEL_DRAG_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn toplevel_drag_manager_get_drag(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    data_source: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        if data_source.is_null() || ffi::wl_resource_get_client(data_source) != client {
            return;
        }
        let source_rec = ffi::wl_resource_get_user_data(data_source) as *mut DataSourceRec;
        if source_rec.is_null() || !(*source_rec).toplevel_drag.is_null() {
            ffi::wl_resource_post_error(
                manager,
                ffi::XDG_TOPLEVEL_DRAG_MANAGER_V1_ERROR_INVALID_SOURCE,
                c"data_source already used for toplevel drag".as_ptr(),
            );
            return;
        }

        let ver = ffi::wl_resource_get_version(manager);
        let res = ffi::wl_resource_create(client, &ffi::xdg_toplevel_drag_v1_interface, ver, id);
        if res.is_null() {
            return;
        }

        let rec = Box::into_raw(Box::new(ToplevelDragRec {
            state,
            source: data_source,
            attached_toplevel: std::ptr::null_mut(),
            offset: (0, 0),
            ended: false,
        }));

        (*source_rec).toplevel_drag = res;

        ffi::wl_resource_set_implementation(
            res,
            &XDG_TOPLEVEL_DRAG_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(toplevel_drag_resource_destroy),
        );
    }
}

unsafe extern "C" fn toplevel_drag_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut ToplevelDragRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).ended {
            ffi::wl_resource_post_error(
                resource,
                ffi::XDG_TOPLEVEL_DRAG_V1_ERROR_ONGOING_DRAG,
                c"xdg_toplevel_drag_v1.destroy called before drag ended".as_ptr(),
            );
            return;
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn toplevel_drag_attach(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    toplevel: *mut ffi::wl_resource,
    x_offset: i32,
    y_offset: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut ToplevelDragRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).attached_toplevel.is_null() {
            ffi::wl_resource_post_error(
                resource,
                ffi::XDG_TOPLEVEL_DRAG_V1_ERROR_TOPLEVEL_ATTACHED,
                c"a toplevel is already attached to this drag".as_ptr(),
            );
            return;
        }
        if toplevel.is_null() || ffi::wl_resource_get_client(toplevel) != client {
            return;
        }

        (*rec).attached_toplevel = toplevel;
        (*rec).offset = (x_offset, y_offset);

        let state = (*rec).state;
        if !state.is_null() {
            if let Some(drag) = (*state).drag.as_mut()
                && drag.source == (*rec).source
            {
                drag.attached_toplevel = toplevel;
                drag.toplevel_offset = (x_offset, y_offset);

                let (cur_x, cur_y) = match drag.origin_device {
                    crate::DragOriginDevice::Pointer => ((*state).pointer_x, (*state).pointer_y),
                    crate::DragOriginDevice::Touch { .. } => {
                        ((*state).touch_grab_x, (*state).touch_grab_y)
                    }
                };
                let surface_rec = (*state).surface_by_toplevel(toplevel);
                if !surface_rec.is_null() {
                    crate::reposition_toplevel_with_popups(
                        surface_rec,
                        tessera_model::Point {
                            x: cur_x as i32 - x_offset,
                            y: cur_y as i32 - y_offset,
                        },
                    );
                }
            }
        }
    }
}

unsafe extern "C" fn toplevel_drag_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut ToplevelDragRec;
        if rec.is_null() {
            return;
        }
        let source = (*rec).source;
        if !source.is_null() {
            let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
            if !source_rec.is_null() && (*source_rec).toplevel_drag == resource {
                (*source_rec).toplevel_drag = std::ptr::null_mut();
            }
        }
        let state = (*rec).state;
        if !state.is_null()
            && let Some(drag) = (*state).drag.as_mut()
            && drag.attached_toplevel == (*rec).attached_toplevel
        {
            drag.attached_toplevel = std::ptr::null_mut();
        }
        drop(Box::from_raw(rec));
    }
}

use super::*;

// ----- presentation-time --------------------------------------------------

static PRESENTATION_IMPL: ffi::wp_presentation_interface_impl =
    ffi::wp_presentation_interface_impl {
        destroy: crate::res_destroy,
        feedback: presentation_feedback,
    };

pub(crate) unsafe extern "C" fn presentation_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::checked_resource_create(
            client,
            &ffi::wp_presentation_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &PRESENTATION_IMPL as *const _ as *const c_void,
            data,
            None,
        );
        // The `clock` event (opcode 0) tells clients which clock the feedback
        // timestamps use. `wp_presentation` has one request (`feedback`); the
        // only event is `clock`, and CLOCK_MONOTONIC is its wire id 1.
        const WP_PRESENTATION_CLOCK: u32 = 0;
        const WL_PRESENTATION_CLOCK_ID_MONOTONIC: u32 = 1;
        ffi::wl_resource_post_event(
            res,
            WP_PRESENTATION_CLOCK,
            WL_PRESENTATION_CLOCK_ID_MONOTONIC,
        );
    }
}

/// `wp_presentation.feedback(surface)`: queue a feedback object on the surface
/// so it can be completed once the frame carrying the surface's next commit
/// reaches the display (ADR-0122, `[INV-PACE-02]`).
///
/// A feedback object that cannot be tied to a live surface — the surface was
/// already destroyed, or the request names no surface — is `discarded`
/// immediately, because it can never be presented.
unsafe extern "C" fn presentation_feedback(
    client: *mut ffi::wl_client,
    _presentation: *mut ffi::wl_resource,
    surface: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let fb =
            ffi::checked_resource_create(client, &ffi::wp_presentation_feedback_interface, 1, id);
        if fb.is_null() {
            return;
        }
        let rec = if surface.is_null() {
            std::ptr::null_mut()
        } else {
            ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec
        };
        if rec.is_null() {
            ffi::wl_resource_set_implementation(fb, std::ptr::null(), std::ptr::null_mut(), None);
            ffi::wl_resource_post_event(fb, ffi::WP_PRESENTATION_FEEDBACK_DISCARDED);
            // No request can ever be answered for a feedback with no surface;
            // post the terminal event and free the object.
            ffi::wl_resource_destroy(fb);
            return;
        }
        // The surface is the feedback object's user data so the destroy hook can
        // unlink it before libwayland frees the resource — a client disconnect
        // must never leave a dangling pointer queued for presentation.
        ffi::wl_resource_set_implementation(
            fb,
            std::ptr::null(),
            rec as *mut c_void,
            Some(feedback_resource_destroy),
        );
        (*rec).pending_presentation_feedback.push(fb);
    }
}

/// Detach a feedback object from the surface that queued it. Runs whenever the
/// resource is destroyed — our own completion, a client teardown, or a client
/// disconnect — so the queue can never hold a freed pointer.
unsafe extern "C" fn feedback_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).presentation_feedback.retain(|&f| f != resource);
        (*rec).pending_presentation_feedback.retain(|&f| f != resource);
    }
}

/// Post `discarded` for every feedback object still queued on the surface and
/// free them. Called when a surface is destroyed or unmapped, because a commit
/// that never reaches the display cannot be reported as presented.
pub(crate) unsafe fn discard_surface_feedback(rec: *mut SurfaceRec) {
    unsafe {
        if rec.is_null() {
            return;
        }
        let active = std::mem::take(&mut (*rec).presentation_feedback);
        let pending = std::mem::take(&mut (*rec).pending_presentation_feedback);
        for fb in active.into_iter().chain(pending) {
            if fb.is_null() {
                continue;
            }
            ffi::wl_resource_set_implementation(fb, std::ptr::null(), std::ptr::null_mut(), None);
            ffi::wl_resource_post_event(fb, ffi::WP_PRESENTATION_FEEDBACK_DISCARDED);
            // The queue is already emptied, so the destroy hook's `retain` is a
            // no-op and the resource cannot dangle in a partially-drained list.
            ffi::wl_resource_destroy(fb);
        }
    }
}

/// Whether any surface is waiting for presentation timing, so the presentation
/// path can skip the walk when nobody is listening.
pub(crate) fn presentation_feedback_pending(state: &State) -> bool {
    state
        .surfaces
        .iter()
        .copied()
        .any(|p| !p.is_null() && unsafe { !(*p).presentation_feedback.is_empty() })
}

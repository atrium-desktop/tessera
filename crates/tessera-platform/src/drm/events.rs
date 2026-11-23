use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum FlipProgress {
    Unrelated,
    Pending,
    Complete,
}

#[cfg(test)]
fn retire_page_flip(pending_flips: &mut HashSet<crtc::Handle>, crtc: crtc::Handle) -> FlipProgress {
    if !pending_flips.remove(&crtc) {
        FlipProgress::Unrelated
    } else if pending_flips.is_empty() {
        FlipProgress::Complete
    } else {
        FlipProgress::Pending
    }
}

/// Build the `InputEvent::PointerMotion` for one relative libinput motion,
/// accumulating the absolute pointer position clamped to the output bounds.
/// The clamped position is only the cursor's visual location: the event keeps
/// libinput's accelerated and unaccelerated deltas verbatim so
/// relative-pointer clients (locked game cameras) keep receiving motion after
/// the absolute position pins at an output edge.
fn relative_motion_event(
    pointer: &mut (f32, f32),
    width: u32,
    height: u32,
    dx: f64,
    dy: f64,
    dx_unaccel: f64,
    dy_unaccel: f64,
) -> InputEvent {
    pointer.0 = (pointer.0 + dx as f32).clamp(0.0, width.saturating_sub(1) as f32);
    pointer.1 = (pointer.1 + dy as f32).clamp(0.0, height.saturating_sub(1) as f32);
    InputEvent::PointerMotion {
        x: pointer.0,
        y: pointer.1,
        dx,
        dy,
        dx_unaccel,
        dy_unaccel,
    }
}

/// Map a normalized absolute-device position onto one output's logical
/// content rectangle.
///
/// `destination` is the fitted KMS destination in output-mode pixels; the
/// source occupies the whole logical source rect, with black bars outside it.
/// A coordinate inside a bar clamps to the nearest content edge. Keeping this
/// arithmetic pure makes the letterbox contract testable without a GPU.
fn map_absolute_to_content(
    x_fraction: f32,
    y_fraction: f32,
    mode: (u32, u32),
    destination: (i32, i32, u32, u32),
    source_size: (u32, u32),
    source_origin: (u32, u32),
    source_scale: f32,
) -> Option<(f32, f32)> {
    let (mode_width, mode_height) = mode;
    if mode_width == 0 || mode_height == 0 {
        return None;
    }
    let (dest_x, dest_y, dest_w, dest_h) = destination;
    let content_x = dest_x as f32 / mode_width as f32;
    let content_y = dest_y as f32 / mode_height as f32;
    let content_w = (dest_w as f32 / mode_width as f32).clamp(0.0, 1.0);
    let content_h = (dest_h as f32 / mode_height as f32).clamp(0.0, 1.0);
    if content_w <= 0.0 || content_h <= 0.0 {
        return None;
    }
    let u = ((x_fraction.clamp(0.0, 1.0) - content_x) / content_w).clamp(0.0, 1.0);
    let v = ((y_fraction.clamp(0.0, 1.0) - content_y) / content_h).clamp(0.0, 1.0);
    let scale = if source_scale.is_finite() && source_scale > 0.0 {
        source_scale
    } else {
        1.0
    };
    let logical_w = source_size.0 as f32 / scale;
    let logical_h = source_size.1 as f32 / scale;
    let origin_x = source_origin.0 as f32 / scale;
    let origin_y = source_origin.1 as f32 / scale;
    Some((origin_x + u * logical_w, origin_y + v * logical_h))
}

impl DrmBackend {
    /// Pump backend events until the flip requirement is satisfied, the
    /// `timeout` deadline expires, or the backend fails. The timeout is an
    /// overall deadline rather than a per-poll budget, so a flood of input
    /// events waking poll early cannot starve the flip wait by restarting
    /// the timeout each round. Returns `false` only when the backend is dead;
    /// a flip that outlives the deadline leaves `pending_flips` non-empty.
    pub(super) fn pump(&mut self, timeout: Option<Duration>, require_flip: bool) -> bool {
        if self.failed {
            return false;
        }
        let deadline = timeout.and_then(|value| std::time::Instant::now().checked_add(value));
        loop {
            if !self.poll_round(poll_ms_remaining(deadline)) {
                return false;
            }
            if !require_flip || self.pending_flips.is_empty() || deadline_passed(deadline) {
                return !self.failed;
            }
        }
    }

    /// One poll + dispatch round over the seat, card, input, hotplug, and
    /// Wayland server fds. Returns `false` when the backend has failed and
    /// the loop must exit.
    pub(super) fn poll_round(&mut self, timeout_ms: i32) -> bool {
        let seat_fd = match self.seat.borrow_mut().get_fd() {
            Ok(fd) => fd.as_raw_fd(),
            Err(error) => {
                log::error!("libseat: cannot get event fd: {error:?}");
                self.failed = true;
                return false;
            }
        };
        let card_fd = self.card().as_fd().as_raw_fd();
        let input_fd = self.input.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1);
        let hotplug_fd = self.hotplug.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1);
        let server_fd = self.wakeup_fd.unwrap_or(-1);
        let mut fds = [
            libc::pollfd {
                fd: seat_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: card_fd,
                events: if self.active { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: input_fd,
                events: if self.active { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: hotplug_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            // Client surface commits wake an idle compositor the same way
            // input does. Readability needs no handling here: the main loop
            // dispatches the server event loop itself after poll returns.
            libc::pollfd {
                fd: server_fd,
                events: if self.active { libc::POLLIN } else { 0 },
                revents: 0,
            },
        ];
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout_ms) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return true;
            }
            log::error!("backend poll failed: {error}");
            self.failed = true;
            return false;
        }

        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            if let Err(error) = self.seat.borrow_mut().dispatch(0) {
                log::error!("libseat dispatch failed: {error:?}");
                self.failed = true;
                return false;
            }
            self.apply_seat_event();
        }

        // GPU unplug/revoke: exiting cleanly beats spinning on a dead card fd.
        if fds[1].revents & (libc::POLLHUP | libc::POLLNVAL) != 0 {
            log::error!("drm: card disconnected or revoked; shutting down backend");
            self.failed = true;
            return false;
        }

        if self.active && fds[1].revents & (libc::POLLIN | libc::POLLERR) != 0 {
            match self.card().receive_events() {
                Ok(events) => {
                    for event in events {
                        if let control::Event::PageFlip(event) = event {
                            let mut retired = None;
                            if let Some(slot) =
                                self.slots.iter_mut().find(|slot| slot.crtc == event.crtc)
                            {
                                slot.pending = false;
                                retired = slot.retiring.take();
                                self.pending_flips.remove(&event.crtc);
                            } else {
                                log::trace!(
                                    "drm: ignoring page flip for unowned CRTC {:?}",
                                    event.crtc
                                );
                            }
                            if let Some(retired) = retired {
                                log::trace!("drm: releasing gfx scanout slot {}", retired.slot);
                                self.release_scanout(retired);
                            }
                            if self.retiring_overlay.as_ref().is_some_and(|overlay| {
                                self.displays.outputs.iter().any(|output| {
                                    output.name == overlay.output && output.crtc == event.crtc
                                })
                            }) && let Some(retired) = self.retiring_overlay.take()
                            {
                                log::trace!("drm: releasing retired overlay scanout");
                                self.release_overlay_scanout(retired);
                            }
                        }
                    }
                }
                Err(error) => {
                    if matches!(error.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)) {
                        // The seat Disable event is still in flight; reading
                        // events from a masterless fd is expected, not fatal.
                        log::warn!("drm: event read while masterless (VT switch); ignoring");
                    } else {
                        log::error!("DRM event read failed: {error}");
                        self.failed = true;
                        return false;
                    }
                }
            }
        }

        if self.active
            && fds[2].revents & (libc::POLLIN | libc::POLLERR) != 0
            && let Some(mut input) = self.input.take()
        {
            if let Err(error) = input.dispatch() {
                log::error!("libinput dispatch failed: {error}");
                self.failed = true;
            } else {
                for event in &mut input {
                    self.push_input_event(event);
                }
            }
            self.input = Some(input);
        }

        if fds[3].revents & (libc::POLLIN | libc::POLLERR) != 0
            && let Some(monitor) = self.hotplug.as_ref()
        {
            let mut saw_event = false;
            for _event in monitor.iter() {
                saw_event = true;
            }
            self.hotplug_pending |= saw_event;
        }
        if self.active && self.hotplug_pending && self.pending_flips.is_empty() {
            self.reconfigure_outputs();
        }

        !self.failed
    }

    pub(super) fn apply_seat_event(&mut self) {
        match self.seat_event.take() {
            Some(PendingSeatEvent::Disable) if self.active => {
                log::info!("libseat: session disabled; suspending input and scanout");
                if let Some(input) = self.input.as_mut() {
                    input.suspend();
                }
                self.active = false;
                if let Err(error) = self.seat.borrow_mut().disable() {
                    log::error!("libseat disable acknowledgement failed: {error:?}");
                    self.failed = true;
                }
            }
            Some(PendingSeatEvent::Enable) if !self.active => {
                log::info!("libseat: session enabled; resuming input and scanout");
                if self
                    .input
                    .as_mut()
                    .is_some_and(|input| input.resume().is_err())
                {
                    log::error!("libinput resume failed");
                    self.failed = true;
                    return;
                }
                self.drain_input_queue();
                // libseat revoked the card fd while another VT owned the
                // seat: every KMS ioctl on it now fails with EACCES, and its
                // GEM handles, framebuffers, and property blobs died with the
                // fd. Close it through the seat and forget the dead records
                // without ioctls (the kernel already freed them), then
                // re-open and re-probe so the next commit runs on a fresh
                // master fd with a TEST_ONLY | ALLOW_MODESET preflight.
                if let Some(card) = self.card.take()
                    && let Err(error) = self.seat.borrow_mut().close_device(card.device)
                {
                    log::warn!(
                        "libseat: failed to close revoked card {}: {error:?}",
                        card.path.display()
                    );
                }
                self.clear_plane_slots(false);
                self.current_overlay = None;
                self.retiring_overlay = None;
                self.pending_flips.clear();
                self.forget_composite_fb_cache();
                // The revoked fd also destroyed every cached cursor dumb buffer,
                // framebuffer, and property blob. Drop their handles without
                // ioctls so the replacement fd never commits or destroys stale
                // numeric IDs.
                self.cursor_buffers.clear();
                self.cursor_state = None;
                self.cursor_plane_active = false;
                self.gamma_blob = None;
                self.hdr_metadata_blob = None;
                self.presented_composite = None;
                self.hotplug_pending = false;
                match open_card_and_outputs(
                    &self.seat,
                    &self.configured_modes,
                    &self.configured_color,
                    &self.configured_icc,
                    self.configured_topology,
                    &self.configured_policies,
                ) {
                    Ok((card, displays)) => {
                        self.card = Some(card);
                        self.surface_stale |= displays.modifiers != self.surface_modifiers;
                        self.pending_resize = Some(Size {
                            w: displays.size.0 as i32,
                            h: displays.size.1 as i32,
                        });
                        self.displays = displays;
                        self.rebuild_plane_slots();
                        // A VT/suspend round-trip always wakes to a freshly
                        // rendered frame. This also closes the edge where the
                        // idle coordinator died while scanout was powered off
                        // and the DRM session was temporarily inactive.
                        self.outputs_powered = true;
                        self.render_ready = true;
                        self.active = true;
                    }
                    Err(error) => {
                        log::error!("drm: failed to re-open the GPU after VT resume: {error}");
                        self.failed = true;
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn push_input_event(&mut self, event: Event) {
        match event {
            Event::Device(DeviceEvent::Added(event)) => {
                self.add_input_device(event.device());
            }
            Event::Device(DeviceEvent::Removed(event)) => {
                self.remove_input_device(&event.device());
            }
            Event::Keyboard(KeyboardEvent::Key(event)) => {
                self.input_events.push(InputEvent::Key {
                    code: event.key(),
                    state: match event.key_state() {
                        input::event::keyboard::KeyState::Pressed => ButtonState::Pressed,
                        input::event::keyboard::KeyState::Released => ButtonState::Released,
                    },
                });
            }
            Event::Pointer(event) => self.push_pointer_event(event),
            Event::Touch(event) => self.push_touch_event(event),
            Event::Gesture(event) => self.push_gesture_event(event),
            Event::Tablet(event) => self.push_tablet_event(event),
            _ => {}
        }
    }

    pub(super) fn drain_input_queue(&mut self) {
        if let Some(mut input) = self.input.take() {
            for event in &mut input {
                self.push_input_event(event);
            }
            self.input = Some(input);
        }
    }

    pub(super) fn is_touchpad(device: &Device) -> bool {
        if !device.has_capability(DeviceCapability::Pointer) {
            return false;
        }
        let methods = device.config_scroll_methods();
        device.config_tap_finger_count() > 0
            || device.config_dwt_is_available()
            || methods.contains(&ScrollMethod::TwoFinger)
            || methods.contains(&ScrollMethod::Edge)
    }

    pub(super) fn add_input_device(&mut self, mut device: Device) {
        let sysname = device.sysname().to_owned();
        let name = device.name().into_owned();
        if Self::is_touchpad(&device) {
            Self::apply_touchpad_profile(&mut device, self.touchpad_config);
            log::info!("libinput: touchpad added: {name} ({sysname})");
            self.touchpads.insert(sysname, device);
        } else if Self::is_mouse(&device) {
            Self::apply_mouse_profile(&mut device, self.mouse_config);
            log::info!("libinput: mouse added: {name} ({sysname})");
            self.mice.insert(sysname, device);
        }
    }

    pub(super) fn remove_input_device(&mut self, device: &Device) {
        let sysname = device.sysname();
        if self.touchpads.remove(sysname).is_some() {
            log::info!("libinput: touchpad removed: {} ({sysname})", device.name());
        } else if self.mice.remove(sysname).is_some() {
            log::info!("libinput: mouse removed: {} ({sysname})", device.name());
        }
    }

    /// A plain mouse: a pointer device that is not a touchpad or tablet tool.
    /// Trackballs and pointing sticks classify here too; they expose the same
    /// libinput acceleration and scroll settings.
    pub(super) fn is_mouse(device: &Device) -> bool {
        device.has_capability(DeviceCapability::Pointer)
            && !device.has_capability(DeviceCapability::TabletTool)
    }

    /// Apply the mouse profile's libinput-backed settings: acceleration and
    /// natural scrolling. The scroll multiplier has no libinput counterpart;
    /// the compositor applies it when translating wheel motion.
    pub(super) fn apply_mouse_profile(device: &mut Device, config: MouseConfig) {
        let name = device.name().into_owned();
        let apply = |setting: &str, result: DeviceConfigResult| {
            if let Err(error) = result {
                log::warn!("libinput: {name}: could not apply {setting}: {error:?}");
            }
        };

        if device.config_scroll_has_natural_scroll() {
            apply(
                "natural scroll",
                device.config_scroll_set_natural_scroll_enabled(config.natural_scroll),
            );
        }
        if device.config_accel_is_available() {
            apply(
                "pointer speed",
                device.config_accel_set_speed(f64::from(config.pointer_speed.clamp(-1.0, 1.0))),
            );
        }
    }

    pub(super) fn apply_touchpad_profile(device: &mut Device, config: TouchpadConfig) {
        let name = device.name().into_owned();
        let apply = |setting: &str, result: DeviceConfigResult| {
            if let Err(error) = result {
                log::warn!("libinput: {name}: could not apply {setting}: {error:?}");
            }
        };

        if device.config_scroll_has_natural_scroll() {
            apply(
                "natural scroll",
                device.config_scroll_set_natural_scroll_enabled(config.natural_scroll),
            );
        }
        if device.config_tap_finger_count() > 0 {
            apply(
                "tap-to-click",
                device.config_tap_set_enabled(config.tap_to_click),
            );
            apply(
                "tap-and-drag",
                device.config_tap_set_drag_enabled(config.tap_and_drag),
            );
            apply(
                "drag lock",
                device.config_tap_set_drag_lock_enabled(if config.drag_lock {
                    DragLockState::EnabledTimeout
                } else {
                    DragLockState::Disabled
                }),
            );
        }
        if device.config_dwt_is_available() {
            apply(
                "disable-while-typing",
                device.config_dwt_set_enabled(config.disable_while_typing),
            );
        }
        if device.config_accel_is_available() {
            apply(
                "pointer speed",
                device.config_accel_set_speed(f64::from(config.pointer_speed.clamp(-1.0, 1.0))),
            );
        }

        let methods = device.config_scroll_methods();
        let requested = match config.scroll_method {
            TouchpadScrollMethod::TwoFinger => ScrollMethod::TwoFinger,
            TouchpadScrollMethod::Edge => ScrollMethod::Edge,
        };
        let selected = methods
            .contains(&requested)
            .then_some(requested)
            .or_else(|| {
                [ScrollMethod::TwoFinger, ScrollMethod::Edge]
                    .into_iter()
                    .find(|method| methods.contains(method))
            });
        if let Some(method) = selected {
            if method != requested {
                log::info!(
                    "libinput: {name}: requested {requested:?} scrolling unsupported; using {method:?}"
                );
            }
            apply("scroll method", device.config_scroll_set_method(method));
        }
    }

    pub(super) fn current_touchpad_status(&self) -> TouchpadStatus {
        let mut capabilities = TouchpadCapabilities::default();
        let mut device_names = Vec::with_capacity(self.touchpads.len());
        for device in self.touchpads.values() {
            let tap = device.config_tap_finger_count() > 0;
            let methods = device.config_scroll_methods();
            capabilities.natural_scroll |= device.config_scroll_has_natural_scroll();
            capabilities.tap_to_click |= tap;
            capabilities.tap_and_drag |= tap;
            capabilities.drag_lock |= tap;
            capabilities.disable_while_typing |= device.config_dwt_is_available();
            capabilities.pointer_speed |= device.config_accel_is_available();
            capabilities.two_finger_scroll |= methods.contains(&ScrollMethod::TwoFinger);
            capabilities.edge_scroll |= methods.contains(&ScrollMethod::Edge);
            device_names.push(device.name().into_owned());
        }
        device_names.sort();
        device_names.dedup();
        TouchpadStatus {
            configurable: true,
            device_names,
            capabilities,
            config: self.touchpad_config,
        }
    }

    pub(super) fn current_mouse_status(&self) -> MouseStatus {
        let mut capabilities = MouseCapabilities::default();
        let mut device_names = Vec::with_capacity(self.mice.len());
        for device in self.mice.values() {
            capabilities.natural_scroll |= device.config_scroll_has_natural_scroll();
            capabilities.pointer_speed |= device.config_accel_is_available();
            device_names.push(device.name().into_owned());
        }
        device_names.sort();
        device_names.dedup();
        MouseStatus {
            configurable: true,
            device_names,
            capabilities,
            config: self.mouse_config,
        }
    }

    pub(super) fn current_input_status(&self) -> InputStatus {
        InputStatus {
            configurable: true,
            touchpad: self.current_touchpad_status(),
            mouse: self.current_mouse_status(),
            // The DRM backend does not track keyboards as configurable
            // devices; the runtime overlays the persisted keyboard profile.
            keyboard: tessera_primitives::input::KeyboardConfig::default(),
            mappings: self.input_mappings.clone(),
        }
    }

    pub(super) fn push_tablet_event(&mut self, event: TabletToolEvent) {
        let (width, height) = self.physical_size();
        let tool = event.tool();
        let kind = match tool.tool_type() {
            Some(TabletToolType::Pen) => 0x140,
            Some(TabletToolType::Eraser) => 0x141,
            Some(TabletToolType::Brush) => 0x142,
            Some(TabletToolType::Pencil) => 0x143,
            Some(TabletToolType::Airbrush) => 0x144,
            Some(TabletToolType::Mouse) => 0x146,
            Some(TabletToolType::Lens) => 0x147,
            _ => 0x140,
        };
        let serial = tool.serial();
        let hardware_id = tool.tool_id();
        let tool_key = if serial != 0 {
            serial
        } else {
            (hardware_id << 16) ^ u64::from(kind)
        };
        match event {
            TabletToolEvent::Proximity(event) => {
                let mut capabilities = 0u32;
                for (present, capability) in [
                    (tool.has_tilt(), 1),
                    (tool.has_pressure(), 2),
                    (tool.has_distance(), 3),
                    (tool.has_rotation(), 4),
                    (tool.has_slider(), 5),
                    (tool.has_wheel(), 6),
                ] {
                    if present {
                        capabilities |= 1 << capability;
                    }
                }
                let x = event.x_transformed(width) as f32;
                let y = event.y_transformed(height) as f32;
                self.input_events.push(InputEvent::Tablet {
                    event: TabletEvent::Proximity {
                        tool: tool_key,
                        info: TabletToolInfo {
                            serial,
                            hardware_id,
                            kind,
                            capabilities,
                        },
                        in_proximity: event.proximity_state() == ProximityState::In,
                        x,
                        y,
                        time: event.time(),
                    },
                });
                if event.proximity_state() == ProximityState::In {
                    self.push_tablet_axes(tool_key, &event, width, height);
                }
            }
            TabletToolEvent::Axis(event) => {
                self.push_tablet_axes(tool_key, &event, width, height);
            }
            TabletToolEvent::Tip(event) => {
                self.push_tablet_axes(tool_key, &event, width, height);
                self.input_events.push(InputEvent::Tablet {
                    event: TabletEvent::Tip {
                        tool: tool_key,
                        state: if event.tip_state() == TipState::Down {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                        time: event.time(),
                    },
                });
            }
            TabletToolEvent::Button(event) => {
                self.push_tablet_button(tool_key, &event);
            }
            // `TabletToolEvent` is non-exhaustive; future axes we do not map
            // yet are ignored rather than breaking the build.
            _ => {}
        }
    }

    pub(super) fn push_tablet_axes<E: TabletToolEventTrait>(
        &mut self,
        tool: u64,
        event: &E,
        width: u32,
        height: u32,
    ) {
        self.input_events.push(InputEvent::Tablet {
            event: TabletEvent::Axes {
                tool,
                x: event.x_transformed(width) as f32,
                y: event.y_transformed(height) as f32,
                pressure: event
                    .pressure_has_changed()
                    .then(|| event.pressure() as f32),
                distance: event
                    .distance_has_changed()
                    .then(|| event.distance() as f32),
                tilt: (event.tilt_x_has_changed() || event.tilt_y_has_changed())
                    .then(|| (event.tilt_x() as f32, event.tilt_y() as f32)),
                rotation: event
                    .rotation_has_changed()
                    .then(|| event.rotation() as f32),
                slider: event
                    .slider_has_changed()
                    .then(|| event.slider_position() as f32),
                wheel: event.wheel_has_changed().then(|| {
                    (
                        event.wheel_delta() as f32,
                        event.wheel_delta_discrete() as i32,
                    )
                }),
                time: event.time(),
            },
        });
    }

    pub(super) fn push_tablet_button(&mut self, tool: u64, event: &TabletToolButtonEvent) {
        self.input_events.push(InputEvent::Tablet {
            event: TabletEvent::Button {
                tool,
                button: event.button(),
                state: match event.button_state() {
                    input::event::pointer::ButtonState::Pressed => ButtonState::Pressed,
                    input::event::pointer::ButtonState::Released => ButtonState::Released,
                },
                time: event.time(),
            },
        });
    }

    /// Resolve a normalized absolute device position through the output it
    /// is bound to. Returns `None` when the device has no binding, so callers
    /// keep the historical full-union mapping.
    ///
    /// The binding is intentionally resolved on every event: hotplug and a
    /// mode/topology change can alter the destination rectangle while the
    /// libinput device handle stays live. The content inset comes from the
    /// fitted mirror destination, so a touch inside a letterbox bar clamps to
    /// the visible edge instead of jumping to a desktop corner.
    fn mapped_absolute_position(
        &self,
        device: &Device,
        x_fraction: f32,
        y_fraction: f32,
    ) -> Option<(f32, f32)> {
        if self.input_mappings.is_empty() {
            return None;
        }
        let name = device.name();
        let sysname = device.sysname();
        let binding = self
            .input_mappings
            .iter()
            .find(|mapping| mapping.device == name || mapping.device == sysname)?;
        let output = self
            .displays
            .outputs
            .iter()
            .find(|output| output.name == binding.output)?;
        let (mode_width, mode_height) = output.mode.size();
        let source_scale = output
            .role
            .mirror_source()
            .and_then(|source| {
                self.displays
                    .outputs
                    .iter()
                    .find(|candidate| candidate.name == source)
            })
            .map_or(output.scale.as_f32(), |source| source.scale.as_f32());
        map_absolute_to_content(
            x_fraction,
            y_fraction,
            (u32::from(mode_width), u32::from(mode_height)),
            output.destination,
            output.source_size,
            (output.x, 0),
            source_scale,
        )
    }

    pub(super) fn push_pointer_event(&mut self, event: PointerEvent) {
        let (width, height) = self.physical_size();
        match event {
            PointerEvent::Motion(event) => {
                let motion = relative_motion_event(
                    &mut self.pointer,
                    width,
                    height,
                    event.dx(),
                    event.dy(),
                    event.dx_unaccelerated(),
                    event.dy_unaccelerated(),
                );
                self.input_events.push(motion);
            }
            PointerEvent::MotionAbsolute(event) => {
                let raw_x = event.absolute_x_transformed(width) as f32;
                let raw_y = event.absolute_y_transformed(height) as f32;
                let (x, y) = self
                    .mapped_absolute_position(
                        &event.device(),
                        raw_x / width.max(1) as f32,
                        raw_y / height.max(1) as f32,
                    )
                    .unwrap_or((raw_x, raw_y));
                // Absolute devices report no delta channel; difference
                // successive positions so relative-pointer clients still see
                // motion. There is no unaccelerated source either.
                let dx = f64::from(x - self.pointer.0);
                let dy = f64::from(y - self.pointer.1);
                self.pointer = (x, y);
                self.input_events.push(InputEvent::PointerMotion {
                    x,
                    y,
                    dx,
                    dy,
                    dx_unaccel: dx,
                    dy_unaccel: dy,
                });
            }
            PointerEvent::Button(event) => {
                self.input_events.push(InputEvent::PointerButton {
                    button: event.button(),
                    state: match event.button_state() {
                        input::event::pointer::ButtonState::Pressed => ButtonState::Pressed,
                        input::event::pointer::ButtonState::Released => ButtonState::Released,
                    },
                });
            }
            PointerEvent::ScrollWheel(event) => {
                self.push_scroll_wheel(&event);
            }
            PointerEvent::ScrollFinger(event) => {
                self.push_scroll_sequence(&event, PointerAxisSource::Finger);
            }
            PointerEvent::ScrollContinuous(event) => {
                self.push_scroll_sequence(&event, PointerAxisSource::Continuous);
            }
            #[allow(deprecated)]
            PointerEvent::Axis(event) => {
                self.push_legacy_scroll(&event);
            }
            _ => {}
        }
    }

    pub(super) fn push_scroll_wheel(&mut self, event: &PointerScrollWheelEvent) {
        // wl_pointer's conventional wheel step is 10 surface units. v120
        // preserves high-resolution wheel fractions without device-angle bias.
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(PointerAxisSource::Wheel),
            ..PointerAxisFrame::default()
        };
        // Wheel events belong to mice and touchpads alike; apply whichever
        // scroll multiplier the reporting device class selects. Tablets and
        // other pointer-capable devices fall back to the mouse profile.
        let factor = self.scroll_factor_for(&event.device());
        frame.horizontal = wheel_axis(event, Axis::Horizontal, factor);
        frame.vertical = wheel_axis(event, Axis::Vertical, factor);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    pub(super) fn push_scroll_sequence<T>(&mut self, event: &T, source: PointerAxisSource)
    where
        T: PointerScrollEvent + PointerEventTrait + EventTrait,
    {
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(source),
            ..PointerAxisFrame::default()
        };
        let inverted = event.device().config_scroll_natural_scroll_enabled();
        let factor = self.scroll_factor_for(&event.device());
        frame.horizontal = sequence_axis(event, Axis::Horizontal, inverted, factor);
        frame.vertical = sequence_axis(event, Axis::Vertical, inverted, factor);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    /// Scroll multiplier for the device that produced an event: touchpads use
    /// the touchpad profile's `scroll_speed`, every other pointer the mouse
    /// profile's. Device handles are stored per sysname, so the lookup
    /// matches the retained libinput `Device`.
    fn scroll_factor_for(&self, device: &Device) -> f32 {
        if self.touchpads.contains_key(device.sysname()) {
            self.touchpad_config.scroll_speed
        } else {
            self.mouse_config.scroll_speed
        }
    }

    #[allow(deprecated)]
    pub(super) fn push_legacy_scroll(&mut self, event: &PointerAxisEvent) {
        let source = match event.axis_source() {
            AxisSource::Wheel => PointerAxisSource::Wheel,
            AxisSource::Finger => PointerAxisSource::Finger,
            AxisSource::Continuous => PointerAxisSource::Continuous,
            AxisSource::WheelTilt => PointerAxisSource::WheelTilt,
        };
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(source),
            ..PointerAxisFrame::default()
        };
        let inverted = event.device().config_scroll_natural_scroll_enabled();
        let factor = self.scroll_factor_for(&event.device());
        frame.horizontal = legacy_axis(event, Axis::Horizontal, source, inverted, factor);
        frame.vertical = legacy_axis(event, Axis::Vertical, source, inverted, factor);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    pub(super) fn push_touch_event(&mut self, event: TouchEvent) {
        let (width, height) = self.physical_size();
        let device = event.device();
        match event {
            TouchEvent::Down(event) => {
                let raw_x = event.x_transformed(width) as f32;
                let raw_y = event.y_transformed(height) as f32;
                let (x, y) = self
                    .mapped_absolute_position(
                        &device,
                        raw_x / width.max(1) as f32,
                        raw_y / height.max(1) as f32,
                    )
                    .unwrap_or((raw_x, raw_y));
                self.input_events.push(InputEvent::TouchDown {
                    id: event.seat_slot() as i32,
                    x,
                    y,
                });
            }
            TouchEvent::Motion(event) => {
                let raw_x = event.x_transformed(width) as f32;
                let raw_y = event.y_transformed(height) as f32;
                let (x, y) = self
                    .mapped_absolute_position(
                        &device,
                        raw_x / width.max(1) as f32,
                        raw_y / height.max(1) as f32,
                    )
                    .unwrap_or((raw_x, raw_y));
                self.input_events.push(InputEvent::TouchMotion {
                    id: event.seat_slot() as i32,
                    x,
                    y,
                });
            }
            TouchEvent::Up(event) => self.input_events.push(InputEvent::TouchUp {
                id: event.seat_slot() as i32,
            }),
            TouchEvent::Frame(_) => self.input_events.push(InputEvent::TouchFrame),
            TouchEvent::Cancel(_) => self.input_events.push(InputEvent::TouchCancel),
            _ => {}
        }
    }

    pub(super) fn push_gesture_event(&mut self, event: GestureEvent) {
        let mapped = match event {
            GestureEvent::Swipe(GestureSwipeEvent::Begin(event)) => {
                Some(PointerGestureEvent::SwipeBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Swipe(GestureSwipeEvent::Update(event)) => {
                Some(PointerGestureEvent::SwipeUpdate {
                    time: event.time(),
                    dx: event.dx() as f32,
                    dy: event.dy() as f32,
                })
            }
            GestureEvent::Swipe(GestureSwipeEvent::End(event)) => {
                Some(PointerGestureEvent::SwipeEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::Begin(event)) => {
                Some(PointerGestureEvent::PinchBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::Update(event)) => {
                Some(PointerGestureEvent::PinchUpdate {
                    time: event.time(),
                    dx: event.dx() as f32,
                    dy: event.dy() as f32,
                    scale: event.scale() as f32,
                    rotation: event.angle_delta() as f32,
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::End(event)) => {
                Some(PointerGestureEvent::PinchEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            GestureEvent::Hold(GestureHoldEvent::Begin(event)) => {
                Some(PointerGestureEvent::HoldBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Hold(GestureHoldEvent::End(event)) => {
                Some(PointerGestureEvent::HoldEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            _ => None,
        };
        if let Some(event) = mapped {
            self.gesture_events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crtc(value: u32) -> crtc::Handle {
        crtc::Handle::from(std::num::NonZeroU32::new(value).unwrap())
    }

    #[test]
    fn atomic_batch_retires_only_after_every_crtc_flips() {
        let first = crtc(1);
        let second = crtc(2);
        let mut pending = HashSet::from([first, second]);

        assert_eq!(retire_page_flip(&mut pending, first), FlipProgress::Pending);
        assert_eq!(
            retire_page_flip(&mut pending, second),
            FlipProgress::Complete
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn unrelated_page_flip_cannot_complete_an_owned_batch() {
        let owned = crtc(1);
        let mut pending = HashSet::from([owned]);

        assert_eq!(
            retire_page_flip(&mut pending, crtc(99)),
            FlipProgress::Unrelated
        );
        assert_eq!(pending, HashSet::from([owned]));
    }

    #[test]
    fn relative_motion_keeps_raw_deltas_when_the_position_clamps() {
        // The pointer sits on the bottom-right edge; further motion clamps the
        // absolute position in place.
        let mut pointer = (1919.0, 1079.0);
        let event = relative_motion_event(&mut pointer, 1920, 1080, 30.0, 25.0, 12.0, 10.0);
        let InputEvent::PointerMotion {
            x,
            y,
            dx,
            dy,
            dx_unaccel,
            dy_unaccel,
        } = event
        else {
            panic!("expected pointer motion");
        };
        assert_eq!((x, y), (1919.0, 1079.0));
        // A locked game camera lives on these deltas: they must survive the
        // edge clamp, with the unaccelerated channel left untouched.
        assert_eq!((dx, dy), (30.0, 25.0));
        assert_eq!((dx_unaccel, dy_unaccel), (12.0, 10.0));
    }

    #[test]
    fn mapped_absolute_input_insets_letterbox_bars() {
        // 1920x1080 source content in a 1280x1024 panel: destination is
        // 1280x720 at y=152. A touch at the panel center maps to the source
        // center; a touch in the top bar clamps to y=0.
        let map = |x: f32, y: f32| {
            map_absolute_to_content(
                x,
                y,
                (1280, 1024),
                (0, 152, 1280, 720),
                (1920, 1080),
                (0, 0),
                1.0,
            )
            .unwrap()
        };
        let (x, y) = map(0.5, 0.5);
        assert!((x - 960.0).abs() < 0.01);
        assert!((y - 540.0).abs() < 0.01);
        let (_, top) = map(0.5, 0.01);
        assert_eq!(top, 0.0);
        let (_, bottom) = map(0.5, 0.99);
        assert_eq!(bottom, 1080.0);
    }

    #[test]
    fn mapped_absolute_input_respects_pillarbox_and_scale() {
        // 16:10 source on a wider panel: 0.9 width centered, source scale 2
        // turns 2560 source pixels into a 1280-wide logical desktop at x=320.
        let (x, y) = map_absolute_to_content(
            0.5,
            0.5,
            (1920, 1080),
            (96, 0, 1728, 1080),
            (2560, 1600),
            (640, 0),
            2.0,
        )
        .unwrap();
        assert!((x - 960.0).abs() < 0.01);
        assert!((y - 400.0).abs() < 0.01);
        let (left, _) = map_absolute_to_content(
            0.01,
            0.5,
            (1920, 1080),
            (96, 0, 1728, 1080),
            (2560, 1600),
            (640, 0),
            2.0,
        )
        .unwrap();
        assert_eq!(left, 320.0);
    }

    #[test]
    fn relative_motion_accumulates_the_absolute_position() {
        let mut pointer = (100.0, 100.0);
        let _ = relative_motion_event(&mut pointer, 1920, 1080, 30.0, 25.0, 12.0, 10.0);
        assert_eq!(pointer, (130.0, 125.0));
        let _ = relative_motion_event(&mut pointer, 1920, 1080, -200.0, -200.0, -80.0, -80.0);
        assert_eq!(pointer, (0.0, 0.0), "position clamps at the top-left edge");
    }
}

pub(crate) use super::motion::detect_cursor_retreat;
use super::*;

impl Chrome for Dock {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let dt = input.as_raw().dt_seconds.max(0.0);
        let cursor = input.as_raw().cursor;
        let down = input.as_raw().mouse_down.first().copied().unwrap_or(false);
        self.last_display = Some((disp.x, disp.y));
        // Idempotent with the pre-pass: whichever runs first resolves the pager,
        // and the other observes the same snapshot.
        self.observe_workspaces(_workspaces);

        // A fullscreen client owns the whole output edge: no animation,
        // handle, hover target, popup, or residual tooltip may surface above
        // it. Maximized windows force the Dock into its collapsed capsule;
        // other non-fullscreen windows use the geometric intersection policy.
        if self.fullscreen_locked() {
            self.dismiss_transient_ui();
            // A fullscreen client covers every pixel the Dock could have
            // vacated, so there is nothing to retire; clearing the budget is
            // also what keeps `damage_pending` from holding the frame clock
            // alive while the Dock is not painting at all.
            self.menu_retire_frames = 0;
            self.menu_damage_rect = None;
            self.motion.reveal = SpringState::new(0.0);
            self.autohide_idle = self.autohide_timeout;
            self.anim_active = false;
            self.prev_down = down;
            return;
        }

        // The optimistic half of a reorder commit: apply the dragged order
        // locally so the strip never waits on the catalog round-trip. The
        // reconciling catalog push clears the flag in `update_app_catalog`.
        self.apply_pending_order();

        let dock_obscured = self.obscured_by_windows(windows, (disp.x, disp.y));
        self.set_dock_obscured(dock_obscured);

        if !self.autohide_stepped_in_prepass {
            let drag_active = self.press.as_ref().is_some_and(|press| press.dragging);
            self.step_autohide_motion(dt, (cursor.x, cursor.y), (disp.x, disp.y), drag_active);
        }
        self.autohide_stepped_in_prepass = false;

        let menu_was_open = self.app_menu.is_open();

        // The Launchpad tile always leads the strip (macOS-style), followed by
        // the pinned apps and any unpinned running windows. The strip is
        // workspace-global: it is built from the retained `all_windows`
        // snapshot, so pinned running-state and transient tiles reflect every
        // workspace, and clicking a tile on another workspace jumps to it.
        // The strip comes from the cache shared with `pointer_bounds`, so it
        // is rebuilt only when the window set, the catalog, or the localized
        // label changes.
        let application_label = i18n.text(Message::Applications);
        let tiles: Vec<Tile> = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            Some(application_label),
        )
        .clone();
        let n = tiles.len();
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let position = self.position;
        let vertical = position.is_vertical();
        // The strip's long axis: x for a bottom dock, y for a side dock.
        // All tile layout runs along this axis and maps to x/y at the end.
        let axis_disp = if vertical { disp.y } else { disp.x };
        let cursor_axis = if vertical { cursor.y } else { cursor.x };
        let rest_bounds = self.live_rest_bounds(n, pinned_count, (disp.x, disp.y));

        // Drop eased sizes for tiles no longer present so the map does not
        // grow unbounded across long sessions.
        let live_keys: std::collections::HashSet<&str> =
            tiles.iter().map(|t| t.key.as_str()).collect();
        self.sizes.retain(|key, _| live_keys.contains(key.as_str()));

        // ---- press & drag state machines --------------------------------
        // The press itself is armed after hit-testing below; this block
        // promotes an already-armed press past the drag threshold, drives
        // the live drag preview, and commits on the release edge. Clicks
        // fire on release (never on press) so the threshold gets first say.
        // Only disjoint fields are mutated while the tile strip borrow is
        // live — no `&mut self` calls here.
        let pressed_edge = down && !self.prev_down;
        let released_edge = !down && self.prev_down;
        let drag = self.handle_press_and_drag(
            &tiles,
            down,
            released_edge,
            (cursor.x, cursor.y),
            (disp.x, disp.y),
            axis_disp,
            cursor_axis,
            n,
            pinned_count,
            out,
        );
        let drag_strip = drag.drag_strip;
        let drag_section = drag.drag_section;
        let drag_insert = drag.drag_insert;

        let drag_active = self.press.as_ref().is_some_and(|press| press.dragging);

        let effective_autohide = self.effective_autohide();

        let rest_len = if vertical {
            rest_bounds.h
        } else {
            rest_bounds.w
        };
        let current_panel = if effective_autohide {
            Self::collapsed_panel_rect(position, (disp.x, disp.y), rest_len, self.autohide_reveal())
        } else {
            rest_bounds
        };

        let over_hover_surface = self.hover_surface_contains(cursor.x, cursor.y);

        // Click outside an expanded autohiding dock dismisses it immediately (Escape hatch).
        if effective_autohide && self.autohide_reveal() > 0.05 && pressed_edge {
            let inside_panel = cursor.x >= current_panel.x
                && cursor.y >= current_panel.y
                && cursor.x < current_panel.x + current_panel.w
                && cursor.y < current_panel.y + current_panel.h;
            let inside_menu = self.app_menu.contains(cursor.x, cursor.y, (disp.x, disp.y));
            if !inside_panel && !over_hover_surface && !inside_menu {
                out.scene_changed |= self.dismiss_expanded_autohide();
            }
        }

        // Pointer activation band for magnification. A visible hover surface
        // independently keeps autohide revealed and preserves magnification
        // while the pointer travels from the icon into a live preview card.
        let over_hover_surface = self.hover_surface_contains(cursor.x, cursor.y);
        let over_rest_bounds = cursor.x >= rest_bounds.x
            && cursor.y >= rest_bounds.y
            && cursor.x < rest_bounds.x + rest_bounds.w
            && cursor.y < rest_bounds.y + rest_bounds.h;
        // The resting rectangle must stay inert while collapsed. It is
        // enabled for magnification only after the capsule has begun revealing
        // the Dock. An active drag freezes the wave: the drop preview and the
        // magnification spring would fight over the same slots.
        let in_band = !self.launcher_active
            && !self.collapse_pending
            && !drag_active
            && (over_rest_bounds || over_hover_surface)
            && (!effective_autohide || self.autohide_reveal() >= 0.2);

        let autohide_moving = self.was_autohide_animating;

        // ---- contiguous reflow layout -------------------------------------
        // Unlike a fixed-rest dock, the bar widens to fit the magnified tiles
        // and neighbouring tiles spread apart around the cursor — the classic
        // macOS squeeze-and-lift. Because the total width changes, centres are
        // derived *from* the eased widths rather than from fixed slots, so the
        // cursor → tile mapping tracks the live layout.
        //
        // First ease each tile's size toward its magnification target. A
        // first-seen key springs up from DOCK_TILE_BIRTH (grow-in) instead of
        // snapping to rest size.
        let eased = self.step_magnification_wave(
            &tiles,
            in_band,
            over_hover_surface,
            over_rest_bounds,
            cursor_axis,
            axis_disp,
            n,
            pinned_count,
            dt,
            menu_was_open,
            autohide_moving,
            drag_active,
        );

        let (order, section_slot) = Self::drop_preview_permutation(
            drag_strip,
            drag_section,
            drag_insert,
            n,
            pinned_count,
            &tiles,
        );
        let geometry =
            self.resolve_strip_geometry(&eased, &order, section_slot, axis_disp, (disp.x, disp.y));
        let live_section_gap = geometry.section_gap;
        let surface_progress = geometry.surface_progress;
        let content_progress = geometry.content_progress;
        let panel_rect = geometry.panel_rect;
        let icon_baseline = geometry.icon_baseline;
        // Resolved before `geometry` is partially moved below.
        let reading_rect = self.bento_rect(&geometry, vertical);
        let leading_cells = self.leading_cell_rects(&geometry, vertical);
        let mut icon_rects: Vec<Rect> = geometry.icon_rects;

        // During a reorder drag the dragged tile leaves its slot and floats
        // at the cursor, slightly enlarged; the permutation opened the gap.
        if let Some(dragged) = drag_strip {
            let s = icon_rects[dragged].w * DRAG_LIFT_SCALE;
            icon_rects[dragged] = Rect {
                x: cursor.x - s * 0.5,
                y: cursor.y - s * 0.5,
                w: s,
                h: s,
            };
        }

        // The popup belongs to a tile rather than the pointer coordinate.
        // Re-anchor every frame so it follows the tile's live spring geometry.
        out.scene_changed |= self.reanchor_app_menu(&tiles, &icon_rects);

        // The bar and collapsed indicator are the same placed surface, and
        // both are analytic glass bodies: as it drains, the bar morphs into
        // the stadium handle while keeping lensing, tint and its drop shadow.
        // Edge definition comes from the glass rim, not a painted border.
        let panel_thick = if vertical { panel_rect.w } else { panel_rect.h };
        let dwell_progress = (self.autohide_dwell / AUTOHIDE_DWELL_THRESHOLD).clamp(0.0, 1.0);
        let dock_material =
            collapsing_dock_material(&self.design, surface_progress, panel_thick, dwell_progress);
        // A placed surface with an empty body collapses to ~0 (the rect is
        // only an anchor, not a size); a fixed-size child forces it to the
        // bar size.
        materials::place_floating(
            f,
            "tessera-dock",
            SurfaceRole::Dock,
            &self.design,
            panel_rect,
            1.0,
            dock_material,
            |f| {
                f.column_ex(&sized(panel_rect.w, panel_rect.h), |_| {});
            },
        );

        if let Some(rect) = reading_rect {
            crate::components::dock::bento::render_workspace_rail(
                f,
                &self.design,
                "tessera-dock-workspace-rail",
                rect,
                self.workspace_tick_count,
                self.workspace_current,
                self.hovered_workspace_tick,
                content_progress,
                vertical,
            );
        }

        // Hit-test only content that still exists in the morphing surface.
        // The resting bounds remain the outer ownership limit, while the
        // current panel and transformed icon geometry prevent the collapsed
        // handle (or an icon's former position) from impersonating a tile.
        // An active drag owns the pointer: no hover, click, or menu target.
        let previous_hit = self
            .hovered_tile
            .as_ref()
            .and_then(|key| tiles.iter().position(|tile| &tile.key == key));
        let hit = if drag_active || self.launcher_active {
            None
        } else {
            hit_test_tiles(
                (cursor.x, cursor.y),
                rest_bounds,
                panel_rect,
                content_progress,
                &icon_rects,
                vertical,
                previous_hit,
            )
        };

        let prev_ws_tick = self.hovered_workspace_tick;
        let prev_reading_row = self.hovered_reading_row;
        self.hovered_workspace_tick = None;
        self.hovered_reading_row = None;
        if !drag_active && !self.launcher_active {
            for (cell, rect) in &leading_cells {
                match cell {
                    StripCell::Readings(column) => {
                        if self.hovered_reading_row.is_none() {
                            self.hovered_reading_row =
                                column.row_at(*rect, (cursor.x, cursor.y), vertical);
                        }
                    }
                    StripCell::WorkspacePager(_) => {
                        if self.hovered_workspace_tick.is_none() {
                            self.hovered_workspace_tick =
                                crate::components::dock::bento::hit_test_workspace_rail(
                                    *rect,
                                    (cursor.x, cursor.y),
                                    self.workspace_tick_count,
                                    self.workspace_current,
                                    vertical,
                                );
                        }
                    }
                }
            }
        }
        if self.hovered_workspace_tick != prev_ws_tick
            || self.hovered_reading_row != prev_reading_row
        {
            self.anim_active = true;
            out.scene_changed = true;
        }

        // Draw each tile's icon, then its running dot. Once content has
        // reached the sink, no tile placements remain behind the stadium.
        self.render_strip_content(
            f,
            &tiles,
            &icon_rects,
            drag_strip,
            position,
            icon_baseline,
            content_progress,
            panel_rect,
            &order,
            section_slot,
            live_section_gap,
            axis_disp,
            &eased,
            &geometry.centres,
        );

        let right_pressed = !self.launcher_active
            && input
                .as_raw()
                .mouse_pressed
                .get(1)
                .copied()
                .unwrap_or(false);
        self.handle_pointer_interactions(
            &tiles,
            &icon_rects,
            panel_rect,
            hit,
            !self.launcher_active && pressed_edge,
            !self.launcher_active && released_edge,
            right_pressed,
            drag.suppress_release_click,
            drag.released_target,
            (cursor.x, cursor.y),
            !self.launcher_active && over_rest_bounds,
            content_progress,
            menu_was_open,
            position,
            out,
        );

        // Wheel scroll over workspace rail cycles workspaces smoothly.
        let raw = input.as_raw();
        let scroll_delta = if raw.scroll_y.abs() > 0.001 {
            -raw.scroll_y
        } else if raw.scroll_x.abs() > 0.001 {
            raw.scroll_x
        } else if raw.scroll_pixels_y.abs() > 1.0 {
            -raw.scroll_pixels_y.signum()
        } else if raw.scroll_pixels_x.abs() > 1.0 {
            raw.scroll_pixels_x.signum()
        } else {
            0.0
        };

        if self.hovered_workspace_tick.is_some()
            && scroll_delta.abs() > 0.5
            && !self.workspace_ids.is_empty()
        {
            let n = self.workspace_ids.len();
            let new_idx = if scroll_delta > 0.0 {
                (self.workspace_current + 1).min(n - 1)
            } else {
                self.workspace_current.saturating_sub(1)
            };
            if new_idx != self.workspace_current && new_idx < self.workspace_ids.len() {
                out.switch_workspace = Some(self.workspace_ids[new_idx]);
                self.workspace_current = new_idx;
                self.dock_interacted = true;
                out.scene_changed = true;
            }
        }

        self.update_hover_surfaces(
            f,
            &tiles,
            &icon_rects,
            hit,
            over_hover_surface,
            (cursor.x, cursor.y),
            (disp.x, disp.y),
            position,
        );

        self.app_menu.set_design(self.design);
        // The menu's vacated footprint stays owned until the retirement budget
        // expires, not until the next render: the frame that dismisses the menu
        // only repaints the image it happened to acquire, and the other ring
        // images would otherwise show it again (ADR-0227).
        if !self.app_menu.is_open() && self.menu_retire_frames == 0 {
            self.menu_damage_rect = None;
        }
        let menu_was_open = self.app_menu.is_open();
        let menu_bounds_before = menu_was_open
            .then(|| self.app_menu.bounds((disp.x, disp.y)))
            .flatten();
        self.app_menu.render(f, input, &self.all_windows, i18n, out);
        let menu_now_open = self.app_menu.is_open();
        if !menu_now_open && menu_was_open {
            self.menu_tile = None;
            // `AppMenu::render` closes itself on a click outside it or when its
            // owned windows all vanished. Both are structural changes with no
            // animation and no host snapshot behind them, so the shell has to be
            // told or the popup's pixels stay on screen forever.
            if let Some(bounds) = menu_bounds_before {
                Self::record_menu_damage(&mut self.menu_damage_rect, bounds);
                self.menu_retire_frames = 5;
            }
            out.scene_changed = true;
        } else if menu_was_open && self.menu_retire_frames > 0 {
            self.menu_retire_frames = 0;
        }
        if menu_now_open {
            self.menu_retire_frames = 0;
        }
        if self.menu_retire_frames > 0 {
            self.menu_retire_frames -= 1;
        }
        // The popup follows its tile, and a tile rides the magnification spring.
        // When the anchor rect moves, the pixels at the previous rect still hold
        // the old frame; record that band so the vacated area repaints too. This
        // is the same discipline `record_hover_surface_damage` applies to the
        // tooltip and live-preview popovers.
        if menu_now_open && let Some(bounds) = self.app_menu.bounds((disp.x, disp.y)) {
            let damaging = tessera_primitives::Rect::new(
                bounds.x.floor() as i32,
                bounds.y.floor() as i32,
                bounds.w.ceil().max(1.0) as i32,
                bounds.h.ceil().max(1.0) as i32,
            );
            let moved = self.menu_damage_rect != Some(damaging);
            Self::record_menu_damage(&mut self.menu_damage_rect, bounds);
            // A travelling popup must repaint the band it vacated, which no
            // animation predicate reports: the tile's spring is the animating
            // element, not the menu.
            if moved {
                out.scene_changed = true;
            }
        }

        self.motion.anim_active = self.anim_active;
        self.prev_down = down;
    }

    /// The dock reserves its screen edge so tiled windows do not render under
    /// the bar (ADR-0024 chrome-aware work-area). The magnified-icon overshoot
    /// past the bar is intentionally not reserved — chrome draws over windows.
    fn reserved(&self) -> Reserved {
        if self.effective_autohide() || self.fullscreen_locked() {
            return Reserved::default();
        }
        let extent = (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32;
        match self.position {
            DockPosition::Bottom => Reserved {
                top: 0,
                bottom: extent,
                left: 0,
                right: 0,
            },
            DockPosition::Left => Reserved {
                top: 0,
                bottom: 0,
                left: extent,
                right: 0,
            },
            DockPosition::Right => Reserved {
                top: 0,
                bottom: 0,
                left: 0,
                right: extent,
            },
        }
    }

    fn prepare_frame(&mut self, input: &Input, windows: &[Window], workspaces: &WorkspaceSnapshot) {
        if self.fullscreen_locked() {
            return;
        }
        // The pager's width is part of the bar length, and the pre-pass
        // publishes frame-T geometry the hit-test reads. Observing the snapshot
        // only in `render` would let the pre-pass and the paint measure two
        // different bars on any frame where the workspace count changed.
        self.observe_workspaces(workspaces);
        // Apply the optimistic reorder here too: the pre-pass builds the tile
        // strip and the hover hit-test from `self.apps`, so a pending order that
        // only `render` applied would make the pre-pass publish frame-T geometry
        // for the *previous* ordering on the frame after a drag commit.
        self.apply_pending_order();
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        self.last_display = Some(display);
        let obscured = self.obscured_by_windows(windows, display);
        self.set_dock_obscured(obscured);

        let dt = raw.dt_seconds.max(0.0);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let drag_active = self.press.as_ref().is_some_and(|p| p.dragging);
        self.step_autohide_motion(dt, cursor, display, drag_active);
        self.autohide_stepped_in_prepass = true;
    }

    fn live_preview_presentation(&self) -> Option<LivePreviewPresentation> {
        self.live_preview.clone()
    }

    fn captures_keyboard(&self) -> bool {
        self.app_menu.is_open()
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.dismiss_transient_ui();
            if self.effective_autohide() && self.autohide_reveal() > 0.0 {
                self.request_dismiss();
                self.anim_active = true;
            }
        }
    }

    /// The dock's magnify wave eases over many frames; report it as pending so
    /// the main loop keeps rendering (instead of blocking on the host queue)
    /// until every spring has rested.
    fn anim_pending(&self) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        let effective_autohide = self.effective_autohide();
        let idle_timeout = if self.dock_interacted {
            self.autohide_timeout
        } else {
            AUTOHIDE_QUICK_DISMISS_TIMEOUT
        };
        let locked = self.interaction_locked();
        let target = if effective_autohide && !locked && self.autohide_idle >= idle_timeout {
            0.0
        } else {
            1.0
        };
        let dwell_active =
            self.autohide_dwell > 0.0 && self.autohide_dwell < self.autohide_dwell_threshold;
        let idle_counting = effective_autohide
            && !locked
            && self.autohide_reveal() > 0.001
            && self.autohide_idle > 0.0
            && self.autohide_idle < idle_timeout;
        let drag_active = self.press.as_ref().is_some_and(|p| p.dragging);
        // `settled_drain_frames` and `hover_drain_frames` are *damage* bookkeeping
        // (repaint the swapchain ring slots once more so every in-flight image
        // shows the settled chrome), not animations in flight. Folding them
        // into the frame-production predicate closes a feedback loop: a drain
        // count requests a frame, the frame re-primes the drain, and a fully
        // static dock is repainted at the output refresh rate forever. They
        // remain in the damage footprint; they must not drive the clock.
        self.anim_active
            || idle_counting
            || self.motion.anim_pending(target, dwell_active, drag_active)
    }

    fn damage_pending(&self) -> bool {
        self.menu_retire_frames > 0
    }

    /// The dock answers for its own hover reaction with the footprint it already
    /// publishes for its animations.
    ///
    /// Moving the pointer over the bar is the most common thing that happens to
    /// a desktop, and everything it can change — a magnified tile, the name
    /// bubble, a live preview, the context menu — is a region this component
    /// already reports per frame through [`Chrome::damage_regions`]. Without
    /// this answer the host has to treat "the pointer is over the dock" as "the
    /// whole output may have changed", so a mouse crossing the bar repainted the
    /// entire screen at the panel refresh rate.
    ///
    /// A held drag is the exception: it previews across the strip and can travel
    /// to another screen edge, which the strip footprint does not contain. It
    /// keeps the conservative full-output answer rather than claiming a bound it
    /// cannot honour.
    fn pointer_damage_regions(
        &self,
        _x: f32,
        _y: f32,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        if self.press.as_ref().is_some_and(|press| press.dragging) {
            return None;
        }
        self.damage_regions(windows, display)
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        if self.fullscreen_locked() {
            return None;
        }
        // The magnify wave, reveal morph, and spring overshoot all stay
        // inside the same envelope the backdrop capture uses — a strip one
        // panel thick on the dock's edge, wide enough for every tile at full
        // magnification plus overshoot.
        // Each painted body is its own region. Unioning them would make the strip
        // and a popup above it one box spanning the gap between them, which is
        // exactly the over-repaint the list exists to prevent.
        let mut regions: Vec<Rect> = Vec::new();
        let rect = self.capture_footprint(display);
        let strip = {
            // A painted shadow reaches up to twice its blur radius outside
            // the body. Repaint the full shadow envelope, including the old
            // expanded shadow while the body collapses, and round outward.
            let style = self.design.surfaces.for_role(SurfaceRole::Dock);
            let fringe = 2.0 * style.shadow_blur.max(0.0) + 1.0;
            let offset = style.shadow_offset_y;
            let x0 = (rect.x - fringe).floor().max(0.0);
            let y0 = (rect.y - fringe + offset.min(0.0)).floor().max(0.0);
            let x1 = (rect.x + rect.w + fringe).ceil().min(display.0);
            let y1 = (rect.y + rect.h + fringe + offset.max(0.0))
                .ceil()
                .min(display.1);
            tessera_primitives::Rect::new(
                x0 as i32,
                y0 as i32,
                (x1 - x0).max(0.0) as i32,
                (y1 - y0).max(0.0) as i32,
            )
        };
        Dock::push_damage(
            &mut regions,
            strip.origin.x as f32,
            strip.origin.y as f32,
            strip.size.w as f32,
            strip.size.h as f32,
        );
        if let Some(hover_rect) = self.hover_damage_rect {
            Dock::push_damage(
                &mut regions,
                hover_rect.origin.x as f32,
                hover_rect.origin.y as f32,
                hover_rect.size.w as f32,
                hover_rect.size.h as f32,
            );
        }
        // A live-preview panel animates open above the strip (or beside it
        // for a side dock); its panel rect is computed for presentation each
        // frame, so the union with the strip covers both the panel and the
        // strip it emerges from, including the floating-panel shadow halo.
        if let Some(preview) = self.live_preview_presentation() {
            let style = self.design.surfaces.for_role(SurfaceRole::FloatingPanel);
            let fringe = 2.0 * style.shadow_blur.max(0.0) + style.shadow_offset_y.abs() + 4.0;
            let x0 = (preview.panel.origin.x as f32 - fringe).floor().max(0.0);
            let y0 = (preview.panel.origin.y as f32 - fringe).floor().max(0.0);
            let x1 = (preview.panel.origin.x as f32 + preview.panel.size.w as f32 + fringe)
                .ceil()
                .min(display.0);
            let y1 = (preview.panel.origin.y as f32 + preview.panel.size.h as f32 + fringe)
                .ceil()
                .min(display.1);
            Dock::push_damage(&mut regions, x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        }
        // The tile context menu floats above the strip (or beside it on a side
        // dock) and is part of this component's painted output. `menu_damage_rect`
        // additionally carries the band it vacated while its anchor tile moved, so
        // a travelling popup does not strand its previous frame behind it.
        if let Some(menu) = self.menu_damage_rect {
            Dock::push_damage(
                &mut regions,
                menu.origin.x as f32,
                menu.origin.y as f32,
                menu.size.w as f32,
                menu.size.h as f32,
            );
        }
        // Tooltips and newly-opened live previews float inward from the
        // magnified tile envelope (`rect = self.capture_footprint(display)`)
        // while their dwell fade runs. Measuring outward from `rect` (which
        // already accounts for `DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN`) plus
        // the surface's shadow halo guarantees the top/side cap of the bubble
        // or opening preview panel is never clipped on the first frame before
        // `render()` populates `hover_damage_rect`.
        if self.tooltip_alpha() > 0.001
            && let Some(key) = self.tooltip_tile.as_ref()
        {
            let opens_live_preview = self.live_preview.is_some()
                || Self::frame_tiles(
                    &self.tile_cache,
                    &self.apps,
                    &self.all_apps,
                    &self.icons,
                    self.catalog_revision,
                    &self.all_windows,
                    None,
                )
                .iter()
                .any(|tile| &tile.key == key && !tile.windows.is_empty());
            let reach = if opens_live_preview {
                let style = self.design.surfaces.for_role(SurfaceRole::FloatingPanel);
                let fringe = 2.0 * style.shadow_blur.max(0.0) + style.shadow_offset_y.abs() + 4.0;
                PREVIEW_PANEL_GAP
                    + PREVIEW_PANEL_PAD * 2.0
                    + PREVIEW_CARD_MAX_WIDTH * PREVIEW_ASPECT
                    + PREVIEW_LABEL_HEIGHT
                    + fringe
            } else {
                let style = self.design.surfaces.for_role(SurfaceRole::Tooltip);
                let fringe = 2.0 * style.shadow_blur.max(0.0) + style.shadow_offset_y.abs() + 4.0;
                if self.position.is_vertical() {
                    TOOLTIP_GAP + 224.0 + fringe
                } else {
                    TOOLTIP_BAND + fringe
                }
            };
            let band = match self.position {
                DockPosition::Bottom => {
                    let y1 = (rect.y + rect.h).ceil().min(display.1);
                    let y0 = (rect.y - reach).floor().max(0.0);
                    tessera_primitives::Rect::new(0, y0 as i32, display.0 as i32, (y1 - y0) as i32)
                }
                DockPosition::Left => {
                    let x1 = (rect.x + rect.w + reach).ceil().min(display.0);
                    tessera_primitives::Rect::new(0, 0, x1 as i32, display.1 as i32)
                }
                DockPosition::Right => {
                    let x0 = (rect.x - reach).floor().max(0.0);
                    tessera_primitives::Rect::new(
                        x0 as i32,
                        0,
                        (display.0 - x0).ceil() as i32,
                        display.1 as i32,
                    )
                }
            };
            Dock::push_damage(
                &mut regions,
                band.origin.x as f32,
                band.origin.y as f32,
                band.size.w as f32,
                band.size.h as f32,
            );
        }
        Some(regions)
    }

    fn requires_composition(&self) -> bool {
        !self.fullscreen_locked()
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ReducedMotion(reduced) => self.reduced_motion = reduced,
            ChromeUpdate::Windows(windows) => self.update_windows(windows),
            ChromeUpdate::SpaceUse(space_use) => self.apply_space_use(space_use),
            ChromeUpdate::AllWindows(windows) => self.update_all_windows(windows),
            ChromeUpdate::Scale(scale) => self.scale = scale,
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
                self.app_menu.update(update);
            }
            ChromeUpdate::AppCatalog(catalog) => self.update_app_catalog(catalog),
            ChromeUpdate::SystemStatus(status) => self.system_status = status.clone(),
            ChromeUpdate::LauncherActive(active) => {
                if active && !self.launcher_active {
                    self.dismiss_transient_ui();
                    self.anim_active = true;
                }
                self.launcher_active = active;
            }
            _ => {}
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if self.fullscreen_locked() || self.launcher_active {
            return false;
        }
        // A held drag gesture owns the pointer across the whole output: a
        // reorder drag previews across the strip and an edge drag travels to
        // another screen edge.
        if self.press.as_ref().is_some_and(|press| press.dragging) {
            return true;
        }
        if self.app_menu.contains(x, y, display) {
            return true;
        }
        if self.hover_surface_contains(x, y) {
            return true;
        }
        let rest = self.pointer_bounds(display);
        let effective_autohide = self.effective_autohide();
        if effective_autohide && self.autohide_reveal() <= 0.001 {
            return Self::collapsed_indicator_contains(self.position, (x, y), display);
        }
        let rest_len = if self.position.is_vertical() {
            rest.h
        } else {
            rest.w
        };
        let current_panel = if effective_autohide {
            Self::collapsed_panel_rect(self.position, display, rest_len, self.autohide_reveal())
        } else {
            rest
        };
        Self::live_panel_contains(self.position, (x, y), current_panel, display)
    }

    fn persistent_decoration(&self) -> bool {
        true
    }

    fn minimize_targets(
        &self,
        display: (f32, f32),
        out: &mut Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)>,
    ) {
        out.extend(Dock::minimize_targets(self, display));
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }
}

impl Dock {
    /// Apply the optimistic half of a reorder commit: re-sort the pinned/live
    /// app list locally so the strip never waits on the catalog round-trip.
    ///
    /// Called from both the pre-pass and `render` so the tile order the two
    /// observe is identical on the frame after a drag commit. The reconciling
    /// catalog push clears `pending_order` in `update_app_catalog`; the idempotent
    /// `take` makes a second call a no-op.
    fn apply_pending_order(&mut self) {
        if let Some(order) = self.pending_order.take() {
            self.apps.sort_by_key(|app| {
                order
                    .iter()
                    .position(|id| id == &app.entry.id)
                    .unwrap_or(usize::MAX)
            });
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }
    }

    /// Resolve the live Dock strip layout (see [`StripGeometry`]).
    ///
    /// `order` maps preview slot → tile index, and `section_slot` is the slot
    /// where the transient section begins. Callers pass the drag permutation
    /// when a reorder drag is in flight, otherwise the identity order with the
    /// pinned boundary.
    pub(super) fn resolve_strip_geometry(
        &self,
        eased: &[f32],
        order: &[usize],
        section_slot: usize,
        axis_disp: f32,
        display: (f32, f32),
    ) -> StripGeometry {
        let position = self.position;
        let effective_autohide = self.effective_autohide();
        let n = eased.len();
        // The section boundary replaces the ordinary tile gap with the wider
        // section gap; the boundary slot is inert once it falls past the strip.
        let section_gap = if section_slot < order.len() {
            DOCK_SECTION_GAP
        } else {
            0.0
        };
        let bento = self.strip_cells();
        let bar_len = crate::components::dock::bento::strip_length(
            eased.iter().copied(),
            &bento,
            DOCK_TILE_GAP,
            DOCK_PAD,
        );
        let _ = n;
        let bar_axis_origin = (axis_disp - bar_len) * 0.5;
        let reading_len = DockState::reading_extent(&bento);
        let centres = strip_centres(
            eased,
            order,
            bar_axis_origin + reading_len,
            section_slot,
            section_gap,
        );
        let surface_progress = if effective_autohide {
            Self::collapse_surface_progress(self.autohide_reveal())
        } else {
            1.0
        };
        let content_progress = if effective_autohide {
            Self::collapse_content_progress(self.autohide_reveal())
        } else {
            1.0
        };
        let panel_rect = if effective_autohide {
            Self::collapsed_panel_rect(position, display, bar_len, self.autohide_reveal())
        } else {
            Self::panel_rect_for(position, bar_len, display)
        };
        // Icons are anchored to the baseline strip on the panel's inner side
        // and grow toward the screen centre; as content drains into the
        // collapsed handle they converge on the same edge-centre sink as the
        // panel and their size reaches zero before the stadium settles.
        let icon_baseline = match position {
            DockPosition::Bottom => {
                panel_rect.y + panel_rect.h - DOCK_BASELINE_INSET * content_progress
            }
            DockPosition::Left => panel_rect.x + DOCK_BASELINE_INSET * content_progress,
            DockPosition::Right => {
                panel_rect.x + panel_rect.w - DOCK_BASELINE_INSET * content_progress
            }
        };
        let icon_rects: Vec<Rect> = (0..n)
            .map(|i| {
                let s = (eased[i] * content_progress).max(0.0);
                let centre_axis =
                    axis_disp * 0.5 + (centres[i] - axis_disp * 0.5) * content_progress;
                match position {
                    DockPosition::Bottom => Rect {
                        x: centre_axis - s * 0.5,
                        y: icon_baseline - s,
                        w: s,
                        h: s,
                    },
                    DockPosition::Left => Rect {
                        x: icon_baseline,
                        y: centre_axis - s * 0.5,
                        w: s,
                        h: s,
                    },
                    DockPosition::Right => Rect {
                        x: icon_baseline - s,
                        y: centre_axis - s * 0.5,
                        w: s,
                        h: s,
                    },
                }
            })
            .collect();
        StripGeometry {
            section_gap,
            centres,
            surface_progress,
            content_progress,
            panel_rect,
            icon_baseline,
            icon_rects,
        }
    }

    pub(crate) fn step_autohide_motion(
        &mut self,
        dt: f32,
        cursor: (f32, f32),
        display: (f32, f32),
        drag_active: bool,
    ) {
        if self.fullscreen_locked() {
            return;
        }
        if (self.autohide_reveal() - self.motion.reveal_value()).abs() > 0.002 {
            self.motion.reveal = SpringState::new(self.autohide_reveal());
        }
        let position = self.position;
        let effective_autohide = self.effective_autohide();
        let vertical = position.is_vertical();
        let axis_disp = if vertical { display.1 } else { display.0 };
        let cursor_axis = if vertical { cursor.1 } else { cursor.0 };
        let (n, pinned_count, rest_bounds, owner_centre, tile_keys) = {
            let tiles = Self::frame_tiles(
                &self.tile_cache,
                &self.apps,
                &self.all_apps,
                &self.icons,
                self.catalog_revision,
                &self.all_windows,
                None,
            );
            let n = tiles.len();
            let pinned_count = tiles.iter().filter(|t| t.pinned).count();
            let rest_bounds = self.live_rest_bounds(n, pinned_count, display);
            let owner_centre = self
                .tooltip_tile
                .as_ref()
                .or(self.hovered_tile.as_ref())
                .and_then(|key| tiles.iter().position(|t| &t.key == key))
                .map(|owner_idx| self.live_rest_centre(owner_idx, n, pinned_count, axis_disp));
            let keys = if self.sizes.is_empty() {
                Vec::new()
            } else {
                tiles.iter().map(|t| t.key.clone()).collect()
            };
            (n, pinned_count, rest_bounds, owner_centre, keys)
        };
        let rest_len = if vertical {
            rest_bounds.h
        } else {
            rest_bounds.w
        };
        let current_panel = if effective_autohide {
            Self::collapsed_panel_rect(position, display, rest_len, self.autohide_reveal())
        } else {
            rest_bounds
        };
        let over_hover_surface = self.hover_surface_contains(cursor.0, cursor.1);

        if !Self::collapsed_indicator_contains(position, cursor, display) {
            self.hidden_trigger_armed = true;
            self.autohide_dwell = 0.0;
        }

        let capsule_entry = if self.collapse_pending
            || self.dismiss_latched
            || !effective_autohide
            || self.autohide_reveal() >= 0.2
        {
            if self.autohide_reveal() >= 0.2 {
                self.autohide_dwell = 0.0;
            }
            false
        } else {
            let confirmed = Self::step_hidden_reveal(
                position,
                &mut self.hidden_trigger_armed,
                &mut self.autohide_dwell,
                self.autohide_dwell_threshold,
                cursor,
                display,
                dt,
            );
            if self.autohide_dwell > 0.0 && self.autohide_dwell < self.autohide_dwell_threshold {
                self.anim_active = true;
            }
            confirmed
        };

        let idle_timeout = if self.dock_interacted {
            self.autohide_timeout
        } else {
            AUTOHIDE_QUICK_DISMISS_TIMEOUT
        };

        let expanding = effective_autohide
            && self.autohide_reveal() > 0.001
            && self.autohide_idle < idle_timeout
            && !self.collapse_pending
            && !self.dismiss_latched;

        let over_dock_trigger = !self.collapse_pending
            && !self.dismiss_latched
            && (Self::pointer_keeps_revealed(
                effective_autohide,
                self.autohide_reveal(),
                capsule_entry,
                cursor,
                current_panel,
                rest_bounds,
                position,
                display,
            ) || (expanding
                && Self::expanded_trigger_contains(position, cursor, rest_bounds, display)));

        let retreating = effective_autohide
            && self.autohide_reveal() > 0.001
            && self.autohide_reveal() < 0.999
            && detect_cursor_retreat(position, cursor, self.last_cursor, rest_bounds);
        self.last_cursor = Some(cursor);

        let keeps_revealed = (over_dock_trigger || over_hover_surface) && !retreating;
        let menu_open = self.app_menu.is_open();
        // One resolved veto, consumed here and by `anim_pending`: a gesture the
        // user started outranks the idle timer, so a scrub, a drag, or an open
        // menu cannot be collapsed out from under the pointer.
        let locked = self.interaction_locked();

        if effective_autohide {
            if keeps_revealed || locked {
                self.autohide_idle = 0.0;
            } else if retreating {
                self.autohide_idle = self.autohide_timeout;
                self.dismiss_latched = true;
            } else if self.autohide_reveal() > 0.001 {
                self.autohide_idle += dt;
            }
        }

        let target_reveal = if effective_autohide && !locked && self.autohide_idle >= idle_timeout {
            0.0
        } else {
            1.0
        };

        self.motion
            .advance_reveal(target_reveal, dt, self.reduced_motion);

        if self.launcher_active
            || self.collapse_pending
            || self.dismiss_latched
            || (effective_autohide && self.autohide_reveal() < 0.2)
        {
            if self.hover_surface_bounds.is_some() || self.live_preview.is_some() {
                self.dismiss_hover_surface();
            }
            self.tooltip_tile = None;
            self.reset_tooltip_fade();
        }

        let is_settled = !self.motion.reveal_animating(target_reveal);
        if is_settled && target_reveal == 0.0 {
            self.dock_interacted = false;
            self.collapse_pending = false;
            if !Self::collapsed_indicator_contains(position, cursor, display) {
                self.dismiss_latched = false;
            }
        }
        self.was_autohide_animating = !is_settled;

        // The dismissal block above may have cleared `hover_surface_bounds`.
        // `render` recomputes `over_hover_surface` after its equivalent
        // teardown for exactly this reason; the pre-pass must observe the same
        // post-dismissal value or the two disagree about `in_band` and the
        // magnification wave desyncs for a frame (ADR-0171, ADR-0180).
        let over_hover_surface = self.hover_surface_contains(cursor.0, cursor.1);

        // Advance tile magnification springs in the pre-pass whenever tiles are
        // already initialized so `body_rect` (`visual_panel_bounds`)
        // and `damage_regions` observe the exact same frame-T geometry as `render`.
        let mut unsettled = false;
        if !tile_keys.is_empty() {
            let over_rest_bounds = cursor.0 >= rest_bounds.x
                && cursor.1 >= rest_bounds.y
                && cursor.0 < rest_bounds.x + rest_bounds.w
                && cursor.1 < rest_bounds.y + rest_bounds.h;
            let in_band = !self.launcher_active
                && !self.collapse_pending
                && !drag_active
                && (over_rest_bounds || over_hover_surface)
                && (!effective_autohide || self.autohide_reveal() >= 0.2);
            let magnify_axis = Self::magnification_axis(
                over_hover_surface,
                over_rest_bounds,
                if over_hover_surface && !over_rest_bounds {
                    owner_centre
                } else {
                    None
                },
                cursor_axis,
            );
            let mut eased = Vec::with_capacity(tile_keys.len());
            for (i, key) in tile_keys.iter().enumerate() {
                let target = Self::magnification_target(
                    in_band,
                    magnify_axis,
                    self.live_rest_centre(i, n, pinned_count, axis_disp),
                );
                // A key absent from the spring table only happens when the tile
                // set changed since the table was primed. Seed it with the same
                // birth value `render` uses so the pre-pass geometry matches the
                // frame this tile is first painted, instead of a rest-sized
                // placeholder that would misplace the hover hit-test.
                let state = self.sizes.entry(key.clone()).or_insert(SpringState {
                    value: if menu_open {
                        DOCK_TILE
                    } else {
                        DOCK_TILE_BIRTH
                    },
                    velocity: 0.0,
                });
                if menu_open {
                    state.velocity = 0.0;
                    eased.push(state.value);
                    continue;
                }
                eased.push(Self::spring(state, target, dt, self.reduced_motion));
                let drifting = (state.value - target).abs() > 0.15 || state.velocity.abs() > 0.5;
                unsettled |= drifting;
            }
            // The bubble's owner is released by `render`, which is the pass that
            // knows where the tiles were painted (see the note on the dwell
            // timer below). Nothing here may re-run the hit test: a second
            // resolution from a second copy of the geometry disagreed with the
            // painted frame and released the bubble the moment it appeared.
            if self.was_magnific_animating && !unsettled {
                self.motion.settled_drain_frames = SETTLED_DRAIN_FRAMES;
            }
            self.was_magnific_animating = unsettled;
            self.sizes_stepped_in_prepass = true;
        }

        // Advance the dwell and the bubble's fade in the pre-pass, not in
        // `render`: they are *time*, and `anim_active` is recomputed here before
        // the damage assessment reads it, so an `|=` written during the previous
        // `render` never reaches the frame clock.
        //
        // Which tile is hovered is deliberately **not** resolved here. The hit
        // test needs the frame's painted geometry, and the dock resolves that
        // geometry twice: once in this pre-pass (for the springs and the damage
        // envelope) and once in `render` (for the paint). Those two resolutions
        // must agree, and until they share one source they do not, so the hover
        // is resolved by the pass that actually paints. The timer below is the
        // part that has to be here, because the clock reads it here.
        if !self.app_menu.is_open() {
            if self.hovered_tile.is_some() {
                self.hover_elapsed += dt;
            }
            let wants_tooltip = self.hovered_tile.is_some() && self.hover_elapsed >= TOOLTIP_DWELL;
            if wants_tooltip {
                self.dock_interacted = true;
                self.tooltip_tile.clone_from(&self.hovered_tile);
            }
            let target = if wants_tooltip { 1.0 } else { 0.0 };
            self.motion.advance_tooltip(target, dt, self.reduced_motion);
            if target == 0.0 && self.tooltip_alpha() < 0.001 {
                self.motion.tooltip_alpha = 0.0;
                self.tooltip_tile = None;
            }
        }

        let dwell_active =
            self.autohide_dwell > 0.0 && self.autohide_dwell < self.autohide_dwell_threshold;
        self.anim_active = !is_settled
            || unsettled
            || dwell_active
            || drag_active
            || self.hover_surface_animating();
        self.motion.anim_active = self.anim_active;
    }

    /// Whether the hover dwell timer or the tooltip / live-preview fade is
    /// actively advancing and requires the compositor frame clock to keep ticking.
    fn hover_surface_animating(&self) -> bool {
        if self.app_menu.is_open() {
            return false;
        }
        let tooltip_waiting = self.hovered_tile.is_some() && self.hover_elapsed < TOOLTIP_DWELL;
        let target = if self.tooltip_tile.is_some() {
            1.0
        } else {
            0.0
        };
        let tooltip_fading = (target - self.tooltip_alpha()).abs() > 0.001;
        tooltip_waiting || tooltip_fading
    }

    /// Whether a sub-interaction the user started is holding the dock open.
    ///
    /// Three states veto the collapse, and each is a gesture rather than a
    /// position:
    ///
    /// * a **drag** is in flight (a tile being reordered) — the pointer leaves
    ///   the bar by definition;
    /// * a **popup** owned by the dock is open (the app context menu) — the
    ///   pointer lives inside the popup, not the bar;
    /// * a **control is held** (a press that has not resolved), because a press
    ///   that ends outside the bar is still the same gesture.
    ///
    /// Hover is deliberately absent: a pointer resting on a tile is already
    /// covered by the reveal corridor, and treating presence as a lock would let
    /// a parked pointer hold the dock open forever.
    #[must_use]
    pub fn interaction_locked(&self) -> bool {
        let dragging = self.press.as_ref().is_some_and(|press| press.dragging);
        dragging || self.app_menu.is_open() || self.press.is_some()
    }

    /// The bar's inner baseline: the edge icons grow away from.
    ///
    /// Icons are anchored to the baseline strip on the panel's inner side and
    /// grow toward the screen centre; as content drains into the collapsed
    /// handle they converge on the same edge-centre sink as the panel and their
    /// size reaches zero before the stadium settles.
    fn icon_baseline_for(position: DockPosition, panel_rect: Rect, content_progress: f32) -> f32 {
        match position {
            DockPosition::Bottom => {
                panel_rect.y + panel_rect.h - DOCK_BASELINE_INSET * content_progress
            }
            DockPosition::Left => panel_rect.x + DOCK_BASELINE_INSET * content_progress,
            DockPosition::Right => {
                panel_rect.x + panel_rect.w - DOCK_BASELINE_INSET * content_progress
            }
        }
    }

    /// The leading cells' rects, in strip order, on the bar's own baseline.
    ///
    /// Each cell occupies the span the strip geometry reserved for it, so the
    /// paint and the layout cannot disagree about where the leading section ends
    /// and the first icon begins. The reading column and the pager have the same
    /// outer shape — one span of length, a resting tile of thickness — and differ
    /// only in what they draw inside, which is why one resolver serves both.
    pub(super) fn leading_cell_rects(
        &self,
        geometry: &StripGeometry,
        vertical: bool,
    ) -> Vec<(StripCell, Rect)> {
        let cells = self.strip_cells();
        if cells.is_empty() || geometry.content_progress <= 0.001 {
            return Vec::new();
        }
        let start = if vertical {
            geometry.panel_rect.y + DOCK_PAD
        } else {
            geometry.panel_rect.x + DOCK_PAD
        };
        place_leading_cells(
            cells,
            self.position,
            geometry.icon_baseline,
            DOCK_TILE,
            start,
            geometry.content_progress,
        )
    }

    /// The leading cells' rects at rest, for callers and tests that need to know
    /// where a cell is without running a frame.
    ///
    /// The geometry comes from the same placement rule the frame uses, so a test
    /// asserting a reading's position cannot pass on arithmetic the paint does
    /// not perform.
    #[must_use]
    pub fn rest_leading_cells(&self, display: (f32, f32)) -> Vec<(StripCell, Rect)> {
        let panel = self.visual_panel_bounds(display);
        let vertical = self.position.is_vertical();
        let baseline = Self::icon_baseline_for(self.position, panel, 1.0);
        let start = if vertical {
            panel.y + DOCK_PAD
        } else {
            panel.x + DOCK_PAD
        };
        place_leading_cells(
            self.strip_cells(),
            self.position,
            baseline,
            DOCK_TILE,
            start,
            1.0,
        )
    }

    /// The Bento reading section's rect: one box spanning every leading cell.
    ///
    /// Kept for callers that need the section as a single object (the damage
    /// envelope); the paint places each cell separately through
    /// [`Dock::leading_cell_rects`], because a row hit test inside the reading
    /// column needs the cell's own rect, not the union.
    pub(super) fn bento_rect(&self, geometry: &StripGeometry, vertical: bool) -> Option<Rect> {
        let cells = self.strip_cells();
        if cells.is_empty() || geometry.content_progress <= 0.001 {
            return None;
        }
        let total: f32 = cells.iter().map(|cell| cell.rest_width()).sum::<f32>();
        if total <= 0.0 {
            return None;
        }
        // The reading row is as tall as a resting tile but sits on the same
        // baseline, so the pager reads level with the icons rather than floating.
        let thickness = DOCK_TILE * geometry.content_progress;
        let pad = DOCK_PAD;
        let start = if vertical {
            geometry.panel_rect.y + pad
        } else {
            geometry.panel_rect.x + pad
        };
        Some(match self.position {
            DockPosition::Left => Rect {
                x: geometry.icon_baseline,
                y: start,
                w: thickness,
                h: total,
            },
            DockPosition::Right => Rect {
                x: geometry.icon_baseline - thickness,
                y: start,
                w: thickness,
                h: total,
            },
            DockPosition::Bottom => Rect {
                x: start,
                y: geometry.icon_baseline - thickness,
                w: total,
                h: thickness,
            },
        })
    }

    /// The Dock's animated body rect, resolved once per frame for both the
    /// damage envelope and the paint.
    ///
    /// Autohide morphs the whole panel between the expanded bar and the
    /// collapsed capsule; this is the single source of that geometry, so the
    /// painted body and the damage strip can never disagree.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn body_rect(&self, display: (f32, f32)) -> Option<Rect> {
        if self.fullscreen_locked() {
            return None;
        }
        let expanded = self.visual_panel_bounds(display);
        let expanded_len = if self.position.is_vertical() {
            expanded.h
        } else {
            expanded.w
        };
        Some(if self.effective_autohide() {
            Self::collapsed_panel_rect(self.position, display, expanded_len, self.autohide_reveal())
        } else {
            expanded
        })
    }

    /// The Dock body's painted surface at its current reveal.
    ///
    /// This is the whole material: fill, hairline, corner radius, and the
    /// shadow that separates the bar from the desktop. It morphs with the
    /// capsule — the collapsed state is a small pill rather than a flat
    /// hairline, because a resting edge control has to read as a control.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn body_material(&self, display: (f32, f32)) -> Option<(Rect, LayoutOpts)> {
        let bounds = self.body_rect(display)?;
        let surface_progress = if self.effective_autohide() {
            Self::collapse_surface_progress(self.autohide_reveal())
        } else {
            1.0
        };
        let thick = if self.position.is_vertical() {
            bounds.w
        } else {
            bounds.h
        };
        Some((
            bounds,
            collapsing_dock_material(&self.design, surface_progress, thick, self.autohide_dwell),
        ))
    }

    /// The hover surface (name bubble or live preview panel) as painted
    /// material, at its current fade.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn hover_body_material(&self) -> Option<(Rect, LayoutOpts)> {
        let bounds = self.hover_surface_bounds?;
        if self.tooltip_alpha() <= 0.01 || bounds.w <= 0.0 || bounds.h <= 0.0 {
            return None;
        }
        let design = self.design;
        let is_preview = self.live_preview.is_some();
        let style = design.surfaces.for_role(if is_preview {
            SurfaceRole::FloatingPanel
        } else {
            SurfaceRole::Tooltip
        });
        Some((
            bounds,
            LayoutOpts {
                bg: style.body,
                border: style.border,
                border_width: style.border_width,
                radius: if is_preview {
                    style.radius
                } else {
                    TOOLTIP_HEIGHT * 0.5
                },
                pad: 0.0,
                ..surface_layout()
            },
        ))
    }
}

/// Frame-stable Dock strip geometry, resolved once from the eased tile
/// widths, a preview `order` permutation, and the section boundary.
///
/// Both the compositor pre-pass ([`Dock::step_autohide_motion`], which must
/// publish frame-T geometry to `damage_regions` and `bodies`) and
/// `render` (which paints it) derive their layout from
/// [`Dock::resolve_strip_geometry`], so the panel rect, tile centres, icon
/// rects, and drain progress can never disagree (ADR-0171, ADR-0180).
pub(super) struct StripGeometry {
    pub(super) section_gap: f32,
    pub(super) centres: Vec<f32>,
    pub(super) surface_progress: f32,
    pub(super) content_progress: f32,
    pub(super) panel_rect: Rect,
    pub(super) icon_baseline: f32,
    pub(super) icon_rects: Vec<Rect>,
}

/// The running axis offset of each tile's centre, first to last in preview
/// `order`. `section_slot` is the slot in `order` where the transient section
/// begins: that boundary keeps the wider section gap instead of the ordinary
/// tile gap — the same total the resting geometry (`rest_centre_estimate`,
/// `rest_bounds`, the bar length) assumes, so a settled strip lands exactly
/// on the interaction model's geometry and the section divider keeps equal
/// clearance on both sides.
pub(super) fn strip_centres(
    eased: &[f32],
    order: &[usize],
    bar_axis_origin: f32,
    section_slot: usize,
    section_gap: f32,
) -> Vec<f32> {
    let mut centres = vec![0.0_f32; eased.len()];
    let mut next_axis = bar_axis_origin + DOCK_PAD;
    for (slot, tile_index) in order.iter().copied().enumerate() {
        if slot > 0 {
            let gap = if slot == section_slot {
                section_gap
            } else {
                DOCK_TILE_GAP
            };
            next_axis += gap;
        }
        centres[tile_index] = next_axis + eased[tile_index] * 0.5;
        next_axis += eased[tile_index];
    }
    centres
}

/// Snap a hairline's logical center to the device pixel grid and give it
/// exactly one device pixel of width. At scale ≥ 2 a fractional 1-logical-px
/// line at low alpha rasterises to nothing; snapping keeps the section
/// divider crisp at any scale. At scale 1 this is the same 1 logical px line
/// at (within half a device pixel of) the same center.
pub(super) fn snapped_hairline(center: f32, scale: f32) -> (f32, f32) {
    let scale = scale.max(1.0);
    ((center * scale).round() / scale, 1.0 / scale)
}

/// Nearest-tile hit test along the strip axis (`vertical` picks y for a
/// side dock, x for a bottom dock). Gated by the resting bounds, the live
/// panel, and the content drain, so collapsed geometry owns no tiles.
/// Lay the leading cells out along the strip, one span each.
///
/// Pure and shared by the paint, the row hit test, and the resting-geometry
/// helper, so those three cannot compute different positions for the same cell.
pub(super) fn place_leading_cells(
    cells: Vec<StripCell>,
    position: DockPosition,
    icon_baseline: f32,
    tile_thickness: f32,
    axis_start: f32,
    content_progress: f32,
) -> Vec<(StripCell, Rect)> {
    let thickness = tile_thickness * content_progress;
    if thickness <= 0.0 {
        return Vec::new();
    }
    let mut cursor = axis_start;
    let mut rects = Vec::with_capacity(cells.len());
    let mut placed = false;
    for cell in cells {
        let span = cell.rest_width() * content_progress;
        if span <= 0.0 {
            continue;
        }
        if placed {
            cursor += DOCK_TILE_GAP * content_progress;
        }
        let rect = match position {
            DockPosition::Left => Rect {
                x: icon_baseline,
                y: cursor,
                w: thickness,
                h: span,
            },
            DockPosition::Right => Rect {
                x: icon_baseline - thickness,
                y: cursor,
                w: thickness,
                h: span,
            },
            DockPosition::Bottom => Rect {
                x: cursor,
                y: icon_baseline - thickness,
                w: span,
                h: thickness,
            },
        };
        cursor += span;
        placed = true;
        rects.push((cell, rect));
    }
    rects
}

/// Which tile the pointer is over, with hysteresis so a resting pointer has a
/// stable answer.
///
/// `previous` is the tile the last frame reported. It is not an optimization:
/// it is what makes the function a *function*. Two neighbouring tiles are
/// magnified to the same size around a pointer that sits at their midpoint, so
/// their centres are equidistant to within float noise. Without hysteresis the
/// winner is decided by which side the noise fell on that frame, and the answer
/// alternates. Everything downstream treats a hover *change* as a discrete
/// event — it restarts the name bubble's 0.3 s dwell, releases the previous
/// tile's popover, and requests a repaint — so an alternating answer means the
/// dwell is restarted every other frame and the bubble never appears at all
/// (ADR-0177, ADR-0217). The measured symptom was a bubble that took 121 frames
/// to fade in where its own dwell is 36.
///
/// The incumbent therefore wins every tie within [`HIT_HYSTERESIS_PX`]. A real
/// move — the pointer travelling further than that toward another tile — still
/// changes the answer, because the challenger has to be *meaningfully* closer,
/// not merely closer.
pub(super) fn hit_test_tiles(
    cursor: (f32, f32),
    rest_bounds: Rect,
    panel_rect: Rect,
    content_progress: f32,
    icon_rects: &[Rect],
    vertical: bool,
    previous: Option<usize>,
) -> Option<usize> {
    let contains = |rect: Rect| {
        cursor.0 >= rect.x
            && cursor.1 >= rect.y
            && cursor.0 < rect.x + rect.w
            && cursor.1 < rect.y + rect.h
    };
    if content_progress <= AUTOHIDE_CONTENT_INTERACTION_MIN
        || !contains(rest_bounds)
        || !contains(panel_rect)
    {
        return None;
    }

    let mut hit = None;
    let mut best = f32::MAX;
    let cursor_axis = if vertical { cursor.1 } else { cursor.0 };
    for (i, rect) in icon_rects.iter().enumerate() {
        // Tiles are square, so either dimension reads the axis centre.
        let centre = if vertical {
            rect.y + rect.h * 0.5
        } else {
            rect.x + rect.w * 0.5
        };
        // A tile's hit reach is half the widest the tile can become, plus half
        // the gap. It is a constant of the layout, not a function of any frame.
        //
        // Two requirements pull in opposite directions and the fixed point
        // between them is "no feedback":
        //
        // - The reach must cover the *painted* tile. Two magnified tiles sit
        //   ~10 px apart, and a pointer in that band is over the pair; sizing
        //   from the resting `DOCK_TILE` leaves the band dead.
        // - The reach must not depend on the size this frame paints. That size
        //   is an output of the hover, so a reach derived from it chases the
        //   pointer into the seam (ADR-0177).
        //
        // `DOCK_TILE_MAX` is the ceiling the magnification spring is clamped to,
        // so half of it covers every size the tile can ever paint, and it is a
        // constant — so the hit cannot feed back into it.
        let half = DOCK_TILE_MAX * 0.5 + DOCK_TILE_GAP * 0.5;
        let distance = (cursor_axis - centre).abs();
        if distance > half {
            continue;
        }
        // Hysteresis: the incumbent's distance is discounted, so a challenger
        // has to be clearly nearer to take the hover away.
        let scored = if Some(i) == previous {
            distance - HIT_HYSTERESIS_PX
        } else {
            distance
        };
        if scored < best {
            best = scored;
            hit = Some(i);
        }
    }
    hit
}

/// Whether `entry` is the desktop entry a running `app_id` belongs to. The
/// match mirrors the launcher's running-app heuristic: `StartupWMClass`,
/// the desktop-id stem, or the icon name (case-insensitive).
pub(super) fn entry_matches_app_id(entry: &Entry, app_id: &str) -> bool {
    let want = app_id.to_ascii_lowercase();
    if want.is_empty() {
        return false;
    }
    entry
        .startup_wm_class
        .as_deref()
        .is_some_and(|wm| wm.to_ascii_lowercase() == want)
        || entry
            .id
            .trim_end_matches(".desktop")
            .eq_ignore_ascii_case(app_id)
        || entry
            .icon
            .as_deref()
            .is_some_and(|icon| icon.eq_ignore_ascii_case(app_id))
}

/// A fixed-size, transparent container used to force a placed surface (whose
/// `rect` is only an anchor, not a size) to a known width and height.
fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — the reliable filled-rect
/// primitive (lens paints a container's background at its solved size).
fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}

fn mix_channel(collapsed: u8, expanded: u8, progress: f32) -> u8 {
    let progress = progress.clamp(0.0, 1.0);
    (f32::from(collapsed) + (f32::from(expanded) - f32::from(collapsed)) * progress)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn collapsing_radius(design: &Design, surface_progress: f32, height: f32) -> f32 {
    let surface_progress = surface_progress.clamp(0.0, 1.0);
    let radius = AUTOHIDE_HANDLE_HEIGHT * 0.5
        + (design.radii.panel_lg - AUTOHIDE_HANDLE_HEIGHT * 0.5) * surface_progress;
    radius.min((height * 0.5).max(0.0)).max(0.0)
}

/// Straight (unpremultiplied) channels of a palette colour.
///
/// Palette values are stored premultiplied, but an interpolation between two of
/// them has to run on straight channels and repremultiply once: mixing
/// premultiplied bytes and feeding them back through [`Color::rgba`]
/// premultiplies twice and lands the body darker than either endpoint.
fn straight(color: Color) -> (u8, u8, u8, u8) {
    let (r, g, b, a) = color.components();
    if a == 0 || a == 255 {
        return (r, g, b, a);
    }
    // The quotient is provably <= 255 because `c` is a premultiplied channel
    // (`c <= a`), so the narrowing cannot truncate.
    let un = |c: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
    (un(r), un(g), un(b), a)
}

/// How far the resting capsule's body lifts while the pointer dwells on it.
///
/// The dwell is a micro-interaction acknowledgement, not a second state: it
/// brightens the same fill slightly and resets with the pointer. It is
/// deliberately a tone lift rather than an alpha lift — the capsule is already
/// opaque, so pushing alpha would change nothing visible.
const DOCK_DWELL_GLOW: f32 = 30.0;

fn collapsing_dock_material(
    design: &Design,
    surface_progress: f32,
    height: f32,
    dwell_progress: f32,
) -> LayoutOpts {
    let mut material = materials::panel(design);
    // The painted body interpolates between the two palette endpoints as the
    // surface morphs between the collapsed capsule and the expanded bar.
    let collapsed = straight(design.dock.bar_surface_collapsed);
    let expanded = straight(design.dock.bar_surface_expanded);
    let glow = if surface_progress <= 0.001 {
        (DOCK_DWELL_GLOW * dwell_progress.clamp(0.0, 1.0)).round() as u8
    } else {
        0
    };
    material.bg = Color::rgba(
        mix_channel(collapsed.0, expanded.0, surface_progress).saturating_add(glow),
        mix_channel(collapsed.1, expanded.1, surface_progress).saturating_add(glow),
        mix_channel(collapsed.2, expanded.2, surface_progress).saturating_add(glow),
        mix_channel(collapsed.3, expanded.3, surface_progress),
    );
    material.radius = collapsing_radius(design, surface_progress, height);
    material
}

/// A centred grid row/column with the given gap, for the Launchpad glyph.
fn grid(gap: f32) -> LayoutOpts {
    LayoutOpts {
        gap,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A single icon tile: no fill, no border, no padding — just the raster icon
/// (or glyph fallback), centred so a glyph smaller than the cell is centred.
fn tile_opts() -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        cross: Align::Center,
        ..surface_layout()
    }
}

/// Resolve the name bubble once so the painted foreground and compositor
/// liquid-glass body use identical geometry on the following frame. The
/// bubble opens toward `side` of its owner: above a bottom dock's tile, or
/// into the output beside a side dock's tile.
pub(super) fn tooltip_rect(
    frame: &mut Frame,
    design: &Design,
    label: &str,
    owner: Rect,
    display: (f32, f32),
    _alpha: f32,
    side: PopupSide,
) -> Rect {
    let label = ellipsize(frame, label, design.typography.label, 224.0 - 22.0);
    let text = frame.measure_text(&label, design.typography.label);
    let width = (text.width + 22.0).clamp(54.0, 224.0);
    match side {
        PopupSide::Above => {
            let x = (owner.x + owner.w * 0.5 - width * 0.5)
                .clamp(8.0, (display.0 - width - 8.0).max(8.0));
            let y = (owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT).max(8.0);
            Rect {
                x,
                y,
                w: width,
                h: TOOLTIP_HEIGHT,
            }
        }
        PopupSide::Right | PopupSide::Left => {
            let x = if side == PopupSide::Right {
                (owner.x + owner.w + TOOLTIP_GAP).min((display.0 - width - 8.0).max(8.0))
            } else {
                (owner.x - TOOLTIP_GAP - width).max(8.0)
            };
            let y = (owner.y + owner.h * 0.5 - TOOLTIP_HEIGHT * 0.5)
                .clamp(8.0, (display.1 - TOOLTIP_HEIGHT - 8.0).max(8.0));
            Rect {
                x,
                y,
                w: width,
                h: TOOLTIP_HEIGHT,
            }
        }
    }
}

/// A compact app-name bubble that follows the owning Dock icon. Its physical
/// body comes from the compositor's analytic glass pass; this foreground only
/// supplies a minimal tint and the text.
fn render_tooltip(frame: &mut Frame, design: &Design, label: &str, rect: Rect, alpha: f32) {
    let label = ellipsize(
        frame,
        label,
        design.typography.label,
        (rect.w - 22.0).max(0.0),
    );
    let original = frame.theme();
    frame.set_theme(original.with_fg(design.colors.menu_text));
    frame.set_opacity(alpha);
    let mut material = materials::panel(design);
    // The bubble is one painted body: a rounded fill, a hairline, and the
    // elevation halo that lifts it off the desktop. The fade rides the lens
    // opacity switch, so the halo drains with the body rather than popping.
    material.radius = TOOLTIP_HEIGHT * 0.5;
    materials::place_floating(
        frame,
        "tessera-dock-app-name",
        tessera_design::SurfaceRole::Tooltip,
        design,
        rect,
        alpha,
        material,
        |frame| {
            frame.row_ex(
                &LayoutOpts {
                    height: TOOLTIP_HEIGHT,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| frame.label_compact_sized(&label, design.typography.label),
            );
        },
    );
    frame.set_theme(original);
    frame.set_opacity(1.0);
}

/// Lay out every running window for one Dock application. Typical groups stay
/// in a single row; large groups wrap into a centred grid while preserving a
/// usable card width and staying inside the output margins. The panel opens
/// toward the output interior — above a bottom dock's owner tile, beside a
/// side dock's — and the dock's edge decides which axis bounds the cards.
pub(super) fn live_preview_layout(
    design: &Design,
    display: (f32, f32),
    owner: Rect,
    windows: &[tessera_desktop::window::WindowId],
    visibility: f32,
    position: DockPosition,
) -> LivePreviewPresentation {
    let count = windows.len().max(1);
    // Room for the panel along each axis. For a bottom dock the horizontal
    // room is the output width and the vertical room is the space above the
    // owner; for a side dock the panel sits beside the owner, so the roles
    // swap to the room toward the far edge and the full output height.
    let horizontal_room = match position {
        DockPosition::Bottom => display.0 - PREVIEW_SCREEN_MARGIN * 2.0,
        DockPosition::Left => {
            display.0 - (owner.x + owner.w) - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN * 2.0
        }
        DockPosition::Right => owner.x - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN * 2.0,
    };
    let available_w = horizontal_room.max(1.0);
    let max_columns = (((available_w - PREVIEW_PANEL_PAD * 2.0 + PREVIEW_CARD_GAP)
        / (PREVIEW_CARD_MIN_WIDTH + PREVIEW_CARD_GAP))
        .floor() as usize)
        .clamp(1, count);
    let columns = count.min(max_columns);
    let rows = count.div_ceil(columns);
    let horizontal_width = (available_w
        - PREVIEW_PANEL_PAD * 2.0
        - PREVIEW_CARD_GAP * columns.saturating_sub(1) as f32)
        / columns as f32;
    let vertical_room = match position {
        DockPosition::Bottom => owner.y - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN,
        DockPosition::Left | DockPosition::Right => display.1 - PREVIEW_SCREEN_MARGIN * 2.0,
    };
    let vertical_space = (vertical_room
        - PREVIEW_PANEL_PAD * 2.0
        - PREVIEW_CARD_GAP * rows.saturating_sub(1) as f32)
        .max(1.0);
    let vertical_width =
        ((vertical_space / rows as f32 - PREVIEW_LABEL_HEIGHT) / PREVIEW_ASPECT).max(1.0);
    let card_w = PREVIEW_CARD_MAX_WIDTH
        .min(horizontal_width)
        .min(vertical_width)
        .max(56.0);
    let preview_h = (card_w * PREVIEW_ASPECT).max(44.0);
    let card_h = preview_h + PREVIEW_LABEL_HEIGHT;
    let panel_w = card_w * columns as f32
        + PREVIEW_CARD_GAP * columns.saturating_sub(1) as f32
        + PREVIEW_PANEL_PAD * 2.0;
    let panel_h = card_h * rows as f32
        + PREVIEW_CARD_GAP * rows.saturating_sub(1) as f32
        + PREVIEW_PANEL_PAD * 2.0;
    let panel_x = match position {
        DockPosition::Bottom => (owner.x + owner.w * 0.5 - panel_w * 0.5).clamp(
            PREVIEW_SCREEN_MARGIN,
            (display.0 - panel_w - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN),
        ),
        DockPosition::Left => (owner.x + owner.w + PREVIEW_PANEL_GAP)
            .min((display.0 - panel_w - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN)),
        DockPosition::Right => (owner.x - PREVIEW_PANEL_GAP - panel_w).max(PREVIEW_SCREEN_MARGIN),
    };
    let panel_y = match position {
        DockPosition::Bottom => (owner.y - PREVIEW_PANEL_GAP - panel_h).max(PREVIEW_SCREEN_MARGIN),
        DockPosition::Left | DockPosition::Right => (owner.y + owner.h * 0.5 - panel_h * 0.5)
            .clamp(
                PREVIEW_SCREEN_MARGIN,
                (display.1 - panel_h - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN),
            ),
    };
    let panel = Rect {
        x: panel_x,
        y: panel_y,
        w: panel_w,
        h: panel_h,
    };

    let cards = windows
        .iter()
        .copied()
        .enumerate()
        .map(|(index, window)| {
            let row = index / columns;
            let column = index % columns;
            let row_count = windows.len().saturating_sub(row * columns).min(columns);
            let row_w =
                card_w * row_count as f32 + PREVIEW_CARD_GAP * row_count.saturating_sub(1) as f32;
            let row_x = panel.x + (panel.w - row_w) * 0.5;
            let x = row_x + column as f32 * (card_w + PREVIEW_CARD_GAP);
            let y = panel.y + PREVIEW_PANEL_PAD + row as f32 * (card_h + PREVIEW_CARD_GAP);
            PreviewCard {
                window,
                corner_radius: design.radii.control,
                geometry: crate::layout::window_switcher::Card {
                    outer: core_rect(Rect {
                        x,
                        y,
                        w: card_w,
                        h: card_h,
                    }),
                    preview: core_rect(Rect {
                        x,
                        y,
                        w: card_w,
                        h: preview_h,
                    }),
                    label: core_rect(Rect {
                        x,
                        y: y + preview_h,
                        w: card_w,
                        h: PREVIEW_LABEL_HEIGHT,
                    }),
                },
            }
        })
        .collect();
    LivePreviewPresentation {
        panel: core_rect(panel),
        cards,
        focused: None,
        inactive_content_brightness: design.preview.inactive_content_brightness,
        visibility: visibility.clamp(0.0, 1.0),
    }
}

fn render_live_preview_chrome(
    frame: &mut Frame,
    design: &Design,
    presentation: &LivePreviewPresentation,
    windows: &[Window],
    hovered: Option<tessera_desktop::window::WindowId>,
) {
    let panel = to_lens_rect(presentation.panel);
    let material = preview::panel_material(design);
    // The whole popover fades through the lens opacity switch; per-card
    // content dimming multiplies on top of it below.
    frame.set_opacity(presentation.visibility);
    materials::place_floating(
        frame,
        "tessera-dock-live-previews",
        tessera_design::SurfaceRole::FloatingPanel,
        design,
        panel,
        presentation.visibility,
        material,
        |frame| {
            frame.column_ex(&sized(panel.w, panel.h), |_| {});
        },
    );

    let original = frame.theme();
    frame.set_theme(original.with_fg(design.colors.menu_text));
    for (index, card) in presentation.cards.iter().enumerate() {
        let Some(window) = windows.iter().find(|window| window.id == card.window) else {
            continue;
        };
        let outer = to_lens_rect(card.geometry.outer);
        let is_hovered = hovered == Some(window.id) && !window.read_only;
        let content_brightness = preview::content_brightness(
            presentation.focused,
            window.id,
            presentation.inactive_content_brightness,
        );
        frame.set_opacity(presentation.visibility * content_brightness);
        frame.place(
            &format!("tessera-dock-live-preview-card-{index}"),
            &chrome_place(
                outer,
                preview::card_material(
                    design,
                    if is_hovered {
                        preview::PreviewCardState::Selected
                    } else {
                        preview::PreviewCardState::Rest
                    },
                    card.corner_radius,
                ),
            ),
            |frame| frame.column_ex(&sized(outer.w, outer.h), |_| {}),
        );

        let label_rect = to_lens_rect(card.geometry.label);
        let title = window
            .title
            .as_deref()
            .or(window.app_id.as_deref())
            .unwrap_or("Untitled");
        let label = ellipsize(
            frame,
            title,
            design.typography.label,
            (label_rect.w - 16.0).max(0.0),
        );
        frame.place(
            &format!("tessera-dock-live-preview-label-{index}"),
            &chrome_place(
                label_rect,
                LayoutOpts {
                    bg: Color::TRANSPARENT,
                    radius: design.radii.control,
                    pad: 0.0,
                    cross: Align::Center,
                    ..surface_layout()
                },
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: label_rect.w,
                        height: label_rect.h,
                        pad: 8.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| frame.label_compact_sized(&label, design.typography.label),
                );
            },
        );
    }
    frame.set_theme(original);
    frame.set_opacity(1.0);
}

pub(super) fn live_preview_hit(
    presentation: &LivePreviewPresentation,
    x: f32,
    y: f32,
) -> Option<tessera_desktop::window::WindowId> {
    preview::hit_test(&presentation.cards, None, x, y)
}

fn core_rect(rect: Rect) -> tessera_primitives::Rect {
    tessera_primitives::Rect::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.w.round().max(1.0) as i32,
        rect.h.round().max(1.0) as i32,
    )
}

fn to_lens_rect(rect: tessera_primitives::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

#[derive(Clone, Copy)]
struct Point2D {
    x: f32,
    y: f32,
}

struct DragOutcome {
    suppress_release_click: bool,
    released_target: Option<PressTarget>,
    drag_strip: Option<usize>,
    drag_insert: Option<usize>,
    drag_section: Option<DropSection>,
}

impl Dock {
    fn drop_preview_permutation(
        drag_strip: Option<usize>,
        drag_section: Option<DropSection>,
        drag_insert: Option<usize>,
        n: usize,
        pinned_count: usize,
        tiles: &[Tile],
    ) -> (Vec<usize>, usize) {
        match (drag_strip, drag_section) {
            (Some(dragged), Some(DropSection::Transient)) if tiles[dragged].pinned => {
                let mut order: Vec<usize> = (0..n).filter(|slot| *slot != dragged).collect();
                order.insert((pinned_count - 1).min(order.len()), dragged);
                (order, pinned_count - 1)
            }
            (Some(dragged), Some(DropSection::Pinned)) => {
                let insert = drag_insert.unwrap_or(0);
                let mut order: Vec<usize> = (0..n).filter(|slot| *slot != dragged).collect();
                order.insert((insert + 1).min(order.len()), dragged);
                let section_slot = if tiles[dragged].pinned {
                    pinned_count
                } else {
                    pinned_count + 1
                };
                (order, section_slot)
            }
            _ => ((0..n).collect(), pinned_count),
        }
    }

    fn reanchor_app_menu(&mut self, tiles: &[Tile], icon_rects: &[Rect]) -> bool {
        if self.app_menu.is_open() {
            if let Some((index, _)) = self
                .menu_tile
                .as_ref()
                .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
            {
                self.app_menu.set_owner(icon_rects[index]);
            } else {
                self.app_menu.dismiss();
                self.menu_tile = None;
                return true;
            }
        }
        false
    }

    fn dismiss_expanded_autohide(&mut self) -> bool {
        let had_menu = self.app_menu.is_open();
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.press = None;
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.reset_tooltip_fade();
        if let Some(bounds) = self.hover_surface_bounds.take() {
            Self::record_hover_surface_damage(
                &mut self.hover_damage_rect,
                &mut self.hover_drain_frames,
                bounds,
            );
        }
        self.live_preview = None;
        self.hover_owner_bounds = None;
        self.hovered_preview = None;
        self.dismiss_latched = true;
        self.autohide_idle = self.autohide_timeout;
        self.autohide_dwell = 0.0;
        self.anim_active = true;
        self.motion.anim_active = true;
        had_menu
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_press_and_drag(
        &mut self,
        tiles: &[Tile],
        down: bool,
        released_edge: bool,
        cursor: (f32, f32),
        disp: (f32, f32),
        axis_disp: f32,
        cursor_axis: f32,
        n: usize,
        pinned_count: usize,
        out: &mut ChromeEvents,
    ) -> DragOutcome {
        let cursor = Point2D {
            x: cursor.0,
            y: cursor.1,
        };
        let disp = Point2D {
            x: disp.0,
            y: disp.1,
        };
        let mut suppress_release_click = false;
        let mut released_target: Option<PressTarget> = None;
        let mut drag_strip: Option<usize> = None;
        let mut drag_insert: Option<usize> = None;
        let mut drag_section: Option<DropSection> = None;

        if let Some(mut press) = self.press.take() {
            let mut keep_press = true;
            if down {
                if !press.dragging
                    && Self::drag_threshold_exceeded(press.origin, (cursor.x, cursor.y))
                {
                    press.dragging = match &press.target {
                        PressTarget::PinnedTile(key) => tiles
                            .iter()
                            .any(|tile| &tile.key == key && tile.pinned && tile.app.is_some()),
                        PressTarget::Panel => true,
                        // A transient tile that resolves to a desktop entry
                        // drags across the section divider to pin.
                        PressTarget::OtherTile(key) => tiles.iter().any(|tile| {
                            &tile.key == key && !tile.pinned && tile.pin_entry.is_some()
                        }),
                        PressTarget::WorkspaceTick(_) => false,
                        // A reading is a shortcut, not a control: it never
                        // becomes a drag, so the press cannot reorder anything.
                        PressTarget::ReadingRow(_) => false,
                    };
                }
                if press.dragging {
                    self.dock_interacted = true;
                    self.anim_active = true;
                    match &press.target {
                        PressTarget::PinnedTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            match strip {
                                Some(strip)
                                    if tiles[strip].pinned && tiles[strip].app.is_some() =>
                                {
                                    drag_strip = Some(strip);
                                    let section = Self::drop_section_at(
                                        cursor_axis,
                                        n,
                                        pinned_count,
                                        axis_disp,
                                    );
                                    press.section = section;
                                    drag_section = Some(section);
                                    if section == DropSection::Pinned {
                                        // The insertion slot over the pinned-app
                                        // rest centres, the dragged tile excluded.
                                        let pinned_centres: Vec<f32> = (1..pinned_count)
                                            .filter(|slot| *slot != strip)
                                            .map(|slot| {
                                                self.live_rest_centre(
                                                    slot,
                                                    n,
                                                    pinned_count,
                                                    axis_disp,
                                                )
                                            })
                                            .collect();
                                        let insert =
                                            Self::drop_insert_index(&pinned_centres, cursor_axis);
                                        press.insert = Some(insert);
                                        drag_insert = Some(insert);
                                    } else {
                                        press.insert = None;
                                    }
                                }
                                // The dragged tile vanished mid-gesture (a
                                // window or catalog change): cancel the press.
                                _ => keep_press = false,
                            }
                        }
                        PressTarget::Panel => {
                            if let Some(target) =
                                Self::edge_drag_target((cursor.x, cursor.y), (disp.x, disp.y))
                                && target != self.position
                            {
                                // Inline `set_position`: the strip borrow
                                // rules out `&mut self` calls here.
                                self.position = target;
                                self.app_menu.set_side(Self::popup_side_for(target));
                                self.anim_active = true;
                            }
                        }
                        PressTarget::WorkspaceTick(_) => {}
                        PressTarget::OtherTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            match strip {
                                Some(strip)
                                    if !tiles[strip].pinned && tiles[strip].pin_entry.is_some() =>
                                {
                                    drag_strip = Some(strip);
                                    let section = Self::drop_section_at(
                                        cursor_axis,
                                        n,
                                        pinned_count,
                                        axis_disp,
                                    );
                                    press.section = section;
                                    drag_section = Some(section);
                                    if section == DropSection::Pinned {
                                        // The insertion slot over every
                                        // pinned-app rest centre.
                                        let pinned_centres: Vec<f32> = (1..pinned_count)
                                            .map(|slot| {
                                                self.live_rest_centre(
                                                    slot,
                                                    n,
                                                    pinned_count,
                                                    axis_disp,
                                                )
                                            })
                                            .collect();
                                        let insert =
                                            Self::drop_insert_index(&pinned_centres, cursor_axis);
                                        press.insert = Some(insert);
                                        drag_insert = Some(insert);
                                    } else {
                                        press.insert = None;
                                    }
                                }
                                _ => keep_press = false,
                            }
                        }
                        // A reading never drags; the press stays a pending click
                        // so its release can open the owning surface.
                        PressTarget::ReadingRow(_) => {}
                    }
                }
            }
            if released_edge {
                released_target = Some(press.target.clone());
                if press.dragging {
                    suppress_release_click = true;
                    match &press.target {
                        PressTarget::PinnedTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            if let Some(strip) = strip
                                && strip >= 1
                                && tiles[strip].pinned
                                && let Some(ai) = tiles[strip].app
                            {
                                match press.section {
                                    DropSection::Pinned => {
                                        if let Some(insert) = press.insert {
                                            let mut ids: Vec<String> = self
                                                .apps
                                                .iter()
                                                .map(|app| app.entry.id.clone())
                                                .collect();
                                            // Strip index 0 is the Launchpad; the
                                            // pinned apps sequence starts at 1.
                                            if Self::move_element(&mut ids, strip - 1, insert) {
                                                out.dock_reorder = Some(ids.clone());
                                                self.pending_order = Some(ids);
                                            }
                                        }
                                    }
                                    // Dropped past the divider: unpin. A still
                                    // running app reappears as a transient tile.
                                    DropSection::Transient => {
                                        let id = self.apps[ai].entry.id.clone();
                                        let ids: Vec<String> = self
                                            .apps
                                            .iter()
                                            .map(|app| app.entry.id.clone())
                                            .filter(|entry| entry != &id)
                                            .collect();
                                        self.apps.retain(|app| app.entry.id != id);
                                        out.dock_reorder = Some(ids.clone());
                                        self.pending_order = Some(ids);
                                    }
                                }
                            }
                        }
                        PressTarget::Panel => {
                            if self.position != press.start_position {
                                out.dock_position = Some(self.position);
                            }
                        }
                        PressTarget::OtherTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            // Dropped into the pinned strip: pin at the
                            // previewed slot.
                            if let Some(strip) = strip
                                && !tiles[strip].pinned
                                && press.section == DropSection::Pinned
                                && let (Some(insert), Some(entry_id)) =
                                    (press.insert, tiles[strip].pin_entry.clone())
                                && let Some(entry) = self
                                    .all_apps
                                    .iter()
                                    .find(|entry| entry.id == entry_id)
                                    .cloned()
                            {
                                let pos = insert.min(self.apps.len());
                                let mut ids: Vec<String> =
                                    self.apps.iter().map(|app| app.entry.id.clone()).collect();
                                ids.insert(pos, entry_id);
                                self.apps.insert(
                                    pos,
                                    DockApp {
                                        keys: entry.match_keys(),
                                        entry,
                                    },
                                );
                                out.dock_reorder = Some(ids.clone());
                                self.pending_order = Some(ids);
                            }
                        }
                        PressTarget::WorkspaceTick(_) | PressTarget::ReadingRow(_) => {}
                    }
                }
            } else if keep_press {
                self.press = Some(press);
            }
        }

        DragOutcome {
            suppress_release_click,
            released_target,
            drag_strip,
            drag_insert,
            drag_section,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn step_magnification_wave(
        &mut self,
        tiles: &[Tile],
        in_band: bool,
        over_hover_surface: bool,
        over_rest_bounds: bool,
        cursor_axis: f32,
        axis_disp: f32,
        n: usize,
        pinned_count: usize,
        dt: f32,
        menu_was_open: bool,
        autohide_moving: bool,
        drag_active: bool,
    ) -> Vec<f32> {
        let mut eased: Vec<f32> = Vec::with_capacity(n);
        // Track per-tile (target, velocity) so the anim-pending check below can
        // tell when every spring has fully rested.
        let mut unsettled = false;

        // When the pointer is within the hover surface (popover preview, tooltip,
        // or the air-gap bridge leading to it), lock the magnification axis to the
        // owner tile's resting centre instead of following the live cursor. This
        // keeps the originating app icon at peak magnification as a prominent
        // provenance anchor and prevents the Dock underneath from wiggling or
        // reflowing as the pointer moves across preview cards.
        let owner_centre = if over_hover_surface && !over_rest_bounds {
            self.tooltip_tile
                .as_ref()
                .or(self.hovered_tile.as_ref())
                .and_then(|key| tiles.iter().position(|t| &t.key == key))
                .map(|owner_idx| self.live_rest_centre(owner_idx, n, pinned_count, axis_disp))
        } else {
            None
        };
        let magnify_axis = Self::magnification_axis(
            over_hover_surface,
            over_rest_bounds,
            owner_centre,
            cursor_axis,
        );

        let prestepped = self.sizes_stepped_in_prepass;
        self.sizes_stepped_in_prepass = false;

        for (i, t) in tiles.iter().enumerate() {
            let target = Self::magnification_target(
                in_band,
                magnify_axis,
                self.live_rest_centre(i, n, pinned_count, axis_disp),
            );
            let mut newly_inserted = false;
            // Look up before inserting so an existing tile does not pay a
            // key clone every frame.
            let state = match self.sizes.get_mut(&t.key) {
                Some(state) => state,
                None => {
                    newly_inserted = true;
                    self.sizes.entry(t.key.clone()).or_insert(SpringState {
                        value: if menu_was_open {
                            DOCK_TILE
                        } else {
                            DOCK_TILE_BIRTH
                        },
                        velocity: 0.0,
                    })
                }
            };
            // A context menu must not become a moving target. Freeze the
            // complete wave exactly where it was opened; once the menu closes,
            // the same springs resume toward the live pointer targets.
            if menu_was_open {
                state.velocity = 0.0;
                eased.push(state.value);
                continue;
            }
            if !prestepped || newly_inserted {
                eased.push(Self::spring(state, target, dt, self.reduced_motion));
            } else {
                eased.push(state.value);
            }
            // A spring is still animating while it is meaningfully off its
            // target or still moving. Sub-pixel drift is ignored so we don't
            // tick forever chasing float noise.
            let drifting = (state.value - target).abs() > 0.15 || state.velocity.abs() > 0.5;
            unsettled |= drifting;
        }
        if !prestepped {
            if self.was_magnific_animating && !unsettled {
                self.motion.settled_drain_frames = SETTLED_DRAIN_FRAMES;
            }
            self.was_magnific_animating = unsettled;
        }
        // An active drag keeps frames ticking: the preview follows the cursor
        // even when every spring has rested. Continuous dwell accumulation and
        // hover tooltip / live-preview dwell & fade also tick frames so `render`
        // never clobbers the pre-pass animation state before `update_animation_state`.
        let dwell_active =
            self.autohide_dwell > 0.0 && self.autohide_dwell < self.autohide_dwell_threshold;
        self.anim_active = unsettled
            || autohide_moving
            || drag_active
            || dwell_active
            || self.hover_surface_animating();
        self.motion.anim_active = self.anim_active;

        // The drop-preview permutation: during a tile drag the dragged tile
        // still occupies a strip slot (the bar length is unchanged), but the
        // other tiles shift aside to open the insertion gap at the preview
        // slot. The Launchpad holds slot 0. A pinned tile re-enters within
        // the pinned range; dragged past the divider it previews as the
        // first transient tile; a transient tile dragged into the pinned
        // range previews at the hovered pinned slot. `section_slot` is where
        // the transient section begins in preview order.
        eased
    }

    #[allow(clippy::too_many_arguments)]
    fn render_strip_content(
        &self,
        f: &mut Frame,
        tiles: &[Tile],
        icon_rects: &[Rect],
        drag_strip: Option<usize>,
        position: DockPosition,
        icon_baseline: f32,
        content_progress: f32,
        panel_rect: Rect,
        order: &[usize],
        section_slot: usize,
        live_section_gap: f32,
        axis_disp: f32,
        eased: &[f32],
        centres: &[f32],
    ) {
        let centre = |i: usize| centres[i];
        let dock_colors = self.design.dock;
        if content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            // The drained content fades through the lens opacity switch;
            // restored after the section divider below.
            f.set_opacity(content_progress);
            for (i, t) in tiles.iter().enumerate() {
                let s = icon_rects[i].w;
                let cx = icon_rects[i].x + s * 0.5;
                let cy = icon_rects[i].y + s * 0.5;
                let rect = icon_rects[i];
                let icon_id = format!("tessera-dock-icon-{}", t.key);
                if t.launchpad {
                    // A rounded "app tile" with a 3×3 grid, so it reads as macOS's
                    // Launchpad button. The grid (real content) sizes the surface;
                    // the surface paints the rounded background behind it.
                    let bg = LayoutOpts {
                        bg: dock_colors.launchpad_tile_bg,
                        border: dock_colors.launchpad_tile_border,
                        border_width: 1.0,
                        radius: s * 0.22,
                        pad: s * 0.2,
                        cross: Align::Center,
                        ..surface_layout()
                    };
                    let gap = s * 0.1;
                    let d = (s - 2.0 * (s * 0.2) - 2.0 * gap) / 3.0;
                    // The grid is a subdivided tile: each cell is `d` logical
                    // pixels. While the Dock drains into its collapsed capsule
                    // `s` approaches zero and `d` becomes a sub-pixel sliver.
                    // Snapping such a cell to the device grid can move its
                    // leading edge past its trailing edge, which flux rejects
                    // as a negative rect — and a rejected encoder discards the
                    // whole chrome scene. A cell below one device pixel paints
                    // nothing visible, so do not emit it at all.
                    let cells_visible = d * self.scale >= 1.0;
                    f.place(&icon_id, &chrome_place(rect, bg), |f| {
                        if cells_visible {
                            f.column_ex(&grid(gap), |f| {
                                for _ in 0..3 {
                                    f.row_ex(&grid(gap), |f| {
                                        for _ in 0..3 {
                                            f.column_ex(
                                                &sized_fill(
                                                    d,
                                                    d,
                                                    dock_colors.launchpad_grid,
                                                    d * 0.3,
                                                ),
                                                |_| {},
                                            );
                                        }
                                    });
                                }
                            });
                        }
                    });
                } else {
                    f.place(&icon_id, &chrome_place(rect, tile_opts()), |f| {
                        match t.icon.or_else(|| self.icons.default_icon()) {
                            // The pointer crosses from the binary's flux binding type to
                            // lens's ABI-identical flux_image.
                            Some(ptr) => unsafe {
                                f.image(ptr as *mut lens::sys::flux_image, s, s)
                            },
                            None => f.icon(Icon::FileText, s * 0.6),
                        }
                    });
                }

                if t.running && Some(i) != drag_strip {
                    // Centre the dot in the flat strip between the icon
                    // baseline and the panel's near edge, so it never falls
                    // into the rounded corner region (and outside the bar)
                    // on the first or last tile. On a side dock the strip —
                    // and the stadium's long axis — is vertical.
                    let dot_long = (if t.windows.len() > 1 {
                        DOCK_DOT_STADIUM
                    } else {
                        DOCK_DOT
                    } * content_progress)
                        .max(0.0);
                    let dot_thick = (DOCK_DOT * content_progress).max(0.0);
                    let strip_span =
                        (DOCK_BASELINE_INSET.max(DOCK_DOT) * content_progress).max(0.0);
                    let dot_rect = match position {
                        DockPosition::Bottom => Rect {
                            x: cx - dot_long * 0.5,
                            y: icon_baseline + (strip_span - dot_thick) * 0.5,
                            w: dot_long,
                            h: dot_thick,
                        },
                        DockPosition::Left => Rect {
                            x: icon_baseline + (strip_span - dot_thick) * 0.5,
                            y: cy - dot_long * 0.5,
                            w: dot_thick,
                            h: dot_long,
                        },
                        DockPosition::Right => Rect {
                            x: icon_baseline - strip_span + (strip_span - dot_thick) * 0.5,
                            y: cy - dot_long * 0.5,
                            w: dot_thick,
                            h: dot_long,
                        },
                    };
                    let color = if t.activated {
                        dock_colors.running_dot_active
                    } else {
                        dock_colors.running_dot_inactive
                    };
                    let dot_id = format!("tessera-dock-dot-{}", t.key);
                    f.place(&dot_id, &chrome_place(dot_rect, tile_opts()), |f| {
                        f.column_ex(
                            &sized_fill(dot_rect.w, dot_rect.h, color, dot_thick * 0.5),
                            |_| {},
                        );
                    });
                }
            }
        }

        // A slim divider in the section gap separates the kept strip from
        // the transient running apps, like macOS's Dock. On a side dock the
        // divider lies across the strip: a horizontal hairline. The boundary
        // follows the live preview order, so the divider itself takes part in
        // the reflow when a drag crosses it.
        if live_section_gap > 0.0 && content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            // Sit at the midpoint of the edge-to-edge gap rather than the
            // centre-to-centre midpoint: a boundary tile magnifying toward
            // the divider pushes it aside instead of swallowing it, so the
            // clearance stays one ordinary tile gap on both sides through
            // the wave.
            let before = order[section_slot - 1];
            let after = order[section_slot];
            let divider_axis =
                (centre(before) + eased[before] * 0.5 + centre(after) - eased[after] * 0.5) * 0.5;
            let divider_axis =
                axis_disp * 0.5 + (divider_axis - axis_disp * 0.5) * content_progress;
            let (divider_center, divider_thick) = snapped_hairline(divider_axis, self.scale);
            let divider_len = (DOCK_TILE * 0.55 * content_progress).max(0.0);
            let divider_rect = match position {
                DockPosition::Bottom => Rect {
                    x: divider_center - divider_thick * 0.5,
                    y: panel_rect.y + (panel_rect.h - divider_len) * 0.5,
                    w: divider_thick,
                    h: divider_len,
                },
                DockPosition::Left | DockPosition::Right => Rect {
                    x: panel_rect.x + (panel_rect.w - divider_len) * 0.5,
                    y: divider_center - divider_thick * 0.5,
                    w: divider_len,
                    h: divider_thick,
                },
            };
            f.place(
                "tessera-dock-section-divider",
                &chrome_place(divider_rect, surface_layout()),
                |f| {
                    f.column_ex(
                        &sized_fill(
                            divider_rect.w,
                            divider_rect.h,
                            dock_colors.section_divider,
                            // Deliberately sharp: the divider is a hairline,
                            // and any radius at or below 0.5 falls back to the
                            // same sharp rect path in lens anyway.
                            0.0,
                        ),
                        |_| {},
                    );
                },
            );
        }
        f.set_opacity(1.0);

        // Preview cards keep press-edge activation (no drag gesture starts
        // on them); the per-frame pressed flag is not cleared by the host,
        // so the button-level transition was tracked at the top of render.
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_pointer_interactions(
        &mut self,
        tiles: &[Tile],
        icon_rects: &[Rect],
        panel_rect: Rect,
        hit: Option<usize>,
        pressed_edge: bool,
        released_edge: bool,
        right_pressed: bool,
        suppress_release_click: bool,
        released_target: Option<PressTarget>,
        cursor: (f32, f32),
        over_rest_bounds: bool,
        content_progress: f32,
        menu_was_open: bool,
        position: DockPosition,
        out: &mut ChromeEvents,
    ) {
        let cursor = Point2D {
            x: cursor.0,
            y: cursor.1,
        };
        let previous_hovered_preview = self.hovered_preview;
        let preview_hit = self
            .live_preview
            .as_ref()
            .and_then(|presentation| live_preview_hit(presentation, cursor.x, cursor.y));
        self.hovered_preview = preview_hit.filter(|id| {
            self.all_windows
                .iter()
                .find(|window| window.id == *id)
                .is_some_and(|window| !window.read_only)
        });
        self.anim_active |= self.hovered_preview != previous_hovered_preview;
        let clicked_preview = pressed_edge && !menu_was_open && self.hovered_preview.is_some();
        if clicked_preview {
            out.clicked = self.hovered_preview;
            self.hovered_tile = None;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.reset_tooltip_fade();
            if let Some(bounds) = self.hover_surface_bounds.take() {
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    bounds,
                );
            }
            self.live_preview = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }

        // Arm the press lifecycle: a pinned-app tile press may become a
        // reorder drag; a press on empty panel space may become an edge
        // drag. Anything else stays a pending click.
        if pressed_edge && !menu_was_open && !clicked_preview && self.press.is_none() {
            if let Some(row) = self.hovered_reading_row {
                // A reading is a shortcut, not a control. The press is released
                // immediately as "open the surface that owns the reading": it
                // must not toggle, and it must not drag.
                out.open_control_center = true;
                let _ = row;
                self.dock_interacted = true;
            } else if let Some(ws_idx) = self.hovered_workspace_tick {
                self.press = Some(PressState {
                    origin: (cursor.x, cursor.y),
                    target: PressTarget::WorkspaceTick(ws_idx),
                    dragging: false,
                    section: DropSection::Pinned,
                    insert: None,
                    start_position: position,
                });
            } else if let Some(i) = hit {
                let tile = &tiles[i];
                let target = if tile.pinned && tile.app.is_some() {
                    PressTarget::PinnedTile(tile.key.clone())
                } else {
                    PressTarget::OtherTile(tile.key.clone())
                };
                let section = if tile.pinned {
                    DropSection::Pinned
                } else {
                    DropSection::Transient
                };
                self.press = Some(PressState {
                    origin: (cursor.x, cursor.y),
                    target,
                    dragging: false,
                    section,
                    insert: None,
                    start_position: position,
                });
            } else if over_rest_bounds
                && content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN
                && cursor.x >= panel_rect.x
                && cursor.y >= panel_rect.y
                && cursor.x < panel_rect.x + panel_rect.w
                && cursor.y < panel_rect.y + panel_rect.h
            {
                self.press = Some(PressState {
                    origin: (cursor.x, cursor.y),
                    target: PressTarget::Panel,
                    dragging: false,
                    section: DropSection::Pinned,
                    insert: None,
                    start_position: position,
                });
            }
        }

        // A click fires on the release edge, and only when the press began
        // on the same tile and never became a drag. A release after a drag,
        // or off the pressed tile, activates nothing.
        if released_edge && !suppress_release_click && !menu_was_open {
            if let Some(PressTarget::WorkspaceTick(orig_idx)) = released_target {
                if self.hovered_workspace_tick == Some(orig_idx) {
                    self.dock_interacted = true;
                    if let Some(&target_id) = self.workspace_ids.get(orig_idx) {
                        out.switch_workspace = Some(target_id);
                        self.workspace_current = orig_idx;
                        out.scene_changed = true;
                    }
                }
            } else if let (Some(target), Some(i)) = (released_target, hit) {
                let pressed_this_tile = match &target {
                    PressTarget::PinnedTile(key) | PressTarget::OtherTile(key) => {
                        key == &tiles[i].key
                    }
                    PressTarget::Panel
                    | PressTarget::WorkspaceTick(_)
                    | PressTarget::ReadingRow(_) => false,
                };
                if pressed_this_tile {
                    self.dock_interacted = true;
                    let t = &tiles[i];
                    if t.launchpad {
                        out.toggle_pivot = true;
                    } else if let Some(id) = t.focus {
                        out.clicked = Some(id);
                    } else if let Some(ai) = t.spawn {
                        out.activate_entry(self.apps[ai].entry.clone());
                    }
                }
            }
        }

        // A held left press (pending click or drag) suppresses the menu for
        // its tile; the dragged tile in particular must not pop one up.
        if right_pressed
            && self.press.is_none()
            && let Some(i) = hit
        {
            self.dock_interacted = true;
            let tile = &tiles[i];
            if !tile.launchpad {
                let pin_action = if let Some(ai) = tile.app {
                    // A pinned tile always offers removal from the strip.
                    Some(PinAction::Unpin(self.apps[ai].entry.id.clone()))
                } else {
                    // A transient tile offers "Keep in Dock" when its app
                    // resolved to an enumerated desktop entry at build time.
                    tile.pin_entry.clone().map(PinAction::Pin)
                };
                self.app_menu.open(
                    tile.label.clone(),
                    tile.app.map(|app| self.apps[app].entry.clone()),
                    tile.windows.iter().copied(),
                    icon_rects[i],
                    pin_action,
                );
                self.menu_tile = Some(tile.key.clone());
                // A popup is a structural change with no animation and no
                // host-owned snapshot to carry it: without this the menu opens
                // into the model but never into a rebuilt scene.
                out.scene_changed = true;
            }
        }

        // Reveal an app name or running-window previews after a short dwell.
        // Once open, the popover retains its owner while the pointer crosses
        // the gap above the Dock and enters a preview card.
    }

    #[allow(clippy::too_many_arguments)]
    fn update_hover_surfaces(
        &mut self,
        f: &mut Frame,
        tiles: &[Tile],
        icon_rects: &[Rect],
        hit: Option<usize>,
        over_hover_surface: bool,
        cursor: (f32, f32),
        disp: (f32, f32),
        position: DockPosition,
    ) {
        let cursor = Point2D {
            x: cursor.0,
            y: cursor.1,
        };
        let disp = Point2D {
            x: disp.0,
            y: disp.1,
        };
        // `hovered_tile` and `hover_elapsed` are owned by `prepare_frame`, which
        // resolves the hit test against the geometry this frame paints and
        // advances the dwell on the schedule the frame clock reads (ADR-0171
        // Rule 1). This function paints the consequence: when the pre-pass says
        // the hover moved, the bubble and the preview belonging to the old tile
        // are released here.
        //
        // Writing `hovered_tile` here as well was the defect. `render` runs one
        // phase after the dwell advanced, so the two resolutions disagreed for a
        // frame at a time, each disagreement restarted the 0.3 s dwell, and a
        // resting pointer needed over a second to raise a bubble that is due at
        // 0.3 s.
        // `render` is the writer: it is the pass that knows where the tiles were
        // painted this frame. The dwell and the fade are advanced in
        // `prepare_frame`, where the clock reads them (ADR-0217).
        //
        // The fallback to the popover's owner is the **bridge**: once a tile has
        // raised a popover, the pointer has to be able to travel from the tile
        // into the popover without the hover being lost on the way, or the
        // popover (a live preview especially) is unreachable. `hover_surface_
        // contains` reports the popover and the air gap above the bar, and while
        // the pointer is in that region the owner stays the hover.
        //
        // This fallback is not what made the hover oscillate: that was the
        // `hit_test_tiles` tie between two equally-magnified neighbours, which
        // `HIT_HYSTERESIS_PX` settles. Keeping the fallback and adding the
        // hysteresis are independent, and both are required.
        let hovered_tile = hit.map(|i| tiles[i].key.clone()).or_else(|| {
            over_hover_surface
                .then(|| self.tooltip_tile.clone())
                .flatten()
        });
        if self.hovered_tile != hovered_tile {
            self.hovered_tile = hovered_tile;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.reset_tooltip_fade();
            if let Some(bounds) = self.hover_surface_bounds.take() {
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    bounds,
                );
            }
            self.live_preview = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }

        if self.app_menu.is_open() {
            self.tooltip_tile = None;
            self.reset_tooltip_fade();
            if let Some(bounds) = self.hover_surface_bounds.take() {
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    bounds,
                );
            }
            self.live_preview = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }
        // The bubble's dwell and fade are advanced in `prepare_frame`, before
        // the damage assessment runs (ADR-0217). Advancing them here could not
        // work: `prepare_frame` recomputes `anim_active` on the next frame
        // before anything reads it, so an `|=` written in `render` is
        // unreachable, and the timer gating the bubble never advanced when
        // nothing else was moving.

        if let Some((index, tile)) = self
            .tooltip_tile
            .as_ref()
            .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
        {
            // Once the popover exists, freeze its owner geometry. The Dock's
            // magnification spring can keep settling underneath it, but the
            // compositor preview, glass body, labels, and pointer bridge must
            // remain on one exact rectangle instead of chasing that spring a
            // frame apart.
            let owner = self.hover_owner_bounds.unwrap_or(icon_rects[index]);
            self.hover_owner_bounds = Some(owner);
            let side = Self::popup_side_for(position);
            if tile.windows.is_empty() {
                let rect = tooltip_rect(
                    f,
                    &self.design,
                    &tile.label,
                    owner,
                    (disp.x, disp.y),
                    self.tooltip_alpha(),
                    side,
                );
                self.hover_surface_bounds = Some(rect);
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    rect,
                );
                self.live_preview = None;
                self.hovered_preview = None;
                render_tooltip(f, &self.design, &tile.label, rect, self.tooltip_alpha());
            } else {
                let mut presentation = live_preview_layout(
                    &self.design,
                    (disp.x, disp.y),
                    owner,
                    &tile.windows,
                    self.tooltip_alpha(),
                    position,
                );
                let panel_rect = to_lens_rect(presentation.panel);
                self.hover_surface_bounds = Some(panel_rect);
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    panel_rect,
                );
                let previous_hovered_preview = self.hovered_preview;
                self.hovered_preview =
                    live_preview_hit(&presentation, cursor.x, cursor.y).filter(|id| {
                        self.all_windows
                            .iter()
                            .find(|window| window.id == *id)
                            .is_some_and(|window| !window.read_only)
                    });
                self.anim_active |= self.hovered_preview != previous_hovered_preview;
                presentation.focused = self.hovered_preview;
                render_live_preview_chrome(
                    f,
                    &self.design,
                    &presentation,
                    &self.all_windows,
                    self.hovered_preview,
                );
                self.live_preview = Some(presentation);
            }
        } else {
            if let Some(bounds) = self.hover_surface_bounds.take() {
                Self::record_hover_surface_damage(
                    &mut self.hover_damage_rect,
                    &mut self.hover_drain_frames,
                    bounds,
                );
            }
            self.live_preview = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
            if self.hover_drain_frames > 0 {
                self.hover_drain_frames -= 1;
                if self.hover_drain_frames == 0 {
                    self.hover_damage_rect = None;
                }
            }
        }
        // `update_hover_surfaces` is the authoritative pass that resolves
        // `self.hovered_tile` against the painted tile geometry; fold
        // `hover_surface_animating()` here so `Runtime::update_animation_state`
        // (called at the end of `render_and_present`) sees `anim_pending() == true`
        // on the very frame the pointer lands on a tile or while the bubble fades.
        self.anim_active |= self.hover_surface_animating();
    }
}

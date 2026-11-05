//! Wireless display and casting detail page (ADR-0188, ADR-0192).
//!
//! Renders the out-of-process cast service's `CastStatus` verbatim: the service
//! state, its live sessions, and the sinks it discovered with the reason each one
//! is or is not castable. Chrome never infers a session from a `connected` flag.

use super::*;

impl ControlCenter {
    /// Expanded Cast detail view.
    pub(crate) fn render_cast_detail_view(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        if status.cast.discovery != tessera_desktop::system::CastDiscovery::Scanning {
            self.cast_scanning = false;
        }
        let is_scanning = status.cast.discovery == tessera_desktop::system::CastDiscovery::Scanning
            || self.cast_scanning;
        let service_available =
            status.cast.service == tessera_desktop::system::CastServiceState::Available;

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-cast-detail",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        gap: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        // Header row: [< Back] Title  [↻ Scan]
                        f.row_ex(
                            &LayoutOpts {
                                width: area.w,
                                height: 36.0,
                                cross: Align::Center,
                                gap: 8.0,
                                ..Default::default()
                            },
                            |f| {
                                let (back_resp, _) = f.pressable_row(
                                    "tessera-cast-back-btn",
                                    i18n.text(Message::Back),
                                    &LayoutOpts {
                                        width: 80.0,
                                        height: 36.0,
                                        radius: 10.0,
                                        pad: 6.0,
                                        gap: 4.0,
                                        bg: hud.surface_recessed,
                                        cross: Align::Center,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.icon_raw(
                                            lens::sys::lens_icon_id::LENS_ICON_CHEVRON_LEFT,
                                            16.0,
                                        );
                                        display_label(f, i18n.text(Message::Back), type_scale.body);
                                    },
                                );
                                if back_resp.clicked {
                                    self.cast_expanded = false;
                                    self.cast_scanning = false;
                                    out.system_actions.push(SystemAction::StopCastDiscovery);
                                }

                                f.spacer(4.0);
                                display_label(f, i18n.text(Message::Cast), type_scale.headline);

                                f.flex(1.0);
                                f.spacer(0.0);

                                let (refresh_resp, _) = f.pressable_row(
                                    "tessera-cast-refresh-btn",
                                    "Scan",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 18.0,
                                        cross: Align::Center,
                                        bg: if is_scanning {
                                            hud.selection_surface
                                        } else {
                                            hud.surface_recessed
                                        },
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        f.icon_raw(
                                            lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW,
                                            16.0,
                                        );
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    },
                                );
                                if refresh_resp.clicked {
                                    self.cast_scanning = true;
                                    out.system_actions.push(SystemAction::ScanCast);
                                }
                            },
                        );

                        let list_h = (area.h - 36.0 - 12.0).max(1.0);
                        f.scroll("tessera-cast-scroll", |f| {
                            f.column_ex(
                                &LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    gap: 12.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |f| {
                                    // Section: service state
                                    display_label(f, "Cast Service", type_scale.headline);
                                    let (title, detail) = if service_available {
                                        (
                                            "Cast service ready",
                                            match status.cast.discovery {
                                                tessera_desktop::system::CastDiscovery::Scanning => {
                                                    "Discovering nearby Wi-Fi Display sinks…"
                                                }
                                                tessera_desktop::system::CastDiscovery::Failed => {
                                                    "The last discovery pass failed."
                                                }
                                                tessera_desktop::system::CastDiscovery::Idle => {
                                                    "Not scanning. Press Scan to discover displays."
                                                }
                                            },
                                        )
                                    } else {
                                        (
                                            "Cast service unavailable",
                                            "The out-of-process cast service is not running.",
                                        )
                                    };
                                    f.column_ex(
                                        &LayoutOpts {
                                            width: area.w,
                                            pad: 12.0,
                                            radius: 12.0,
                                            bg: hud.surface_recessed,
                                            border: hud.border,
                                            border_width: 1.0,
                                            cross: Align::Start,
                                            gap: 3.0,
                                            ..Default::default()
                                        },
                                        |f| {
                                            f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                            display_label(f, title, type_scale.body);
                                            f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                            display_label(f, detail, type_scale.footnote);
                                            if let Some(error) = &status.cast.last_error {
                                                display_label(f, error, type_scale.footnote);
                                            }
                                        },
                                    );

                                    // Section: live sessions
                                    f.spacer(4.0);
                                    display_label(f, "Current Session", type_scale.headline);
                                    if status.cast.sessions.is_empty() {
                                        f.column_ex(
                                            &LayoutOpts {
                                                width: area.w,
                                                pad: 12.0,
                                                radius: 12.0,
                                                bg: hud.surface_recessed,
                                                cross: Align::Start,
                                                gap: 3.0,
                                                ..Default::default()
                                            },
                                            |f| {
                                                f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                display_label(
                                                    f,
                                                    "No active display session",
                                                    type_scale.body,
                                                );
                                                f.set_theme(
                                                    themes::hud(&hud).with_fg(hud.text_muted),
                                                );
                                                display_label(
                                                    f,
                                                    "Select a nearby display below to start one.",
                                                    type_scale.footnote,
                                                );
                                            },
                                        );
                                    } else {
                                        for (index, session) in
                                            status.cast.sessions.iter().enumerate()
                                        {
                                            let (row, _) = f.pressable_row(
                                                &format!("tessera-cast-session-{index}"),
                                                &session.sink_name,
                                                &LayoutOpts {
                                                    width: area.w,
                                                    height: 56.0,
                                                    pad: 10.0,
                                                    radius: 12.0,
                                                    bg: hud.selection_surface,
                                                    border: hud.accent.with_alpha(80),
                                                    border_width: 1.0,
                                                    cross: Align::Center,
                                                    gap: 10.0,
                                                    ..Default::default()
                                                },
                                                |f, _| {
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(hud.accent),
                                                    );
                                                    f.icon_raw(
                                                        lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                        20.0,
                                                    );
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(hud.text),
                                                    );
                                                    f.column_ex(
                                                        &LayoutOpts {
                                                            cross: Align::Start,
                                                            gap: 1.0,
                                                            ..Default::default()
                                                        },
                                                        |f| {
                                                            display_label(
                                                                f,
                                                                &session.sink_name,
                                                                type_scale.body,
                                                            );
                                                            f.set_theme(
                                                                themes::hud(&hud)
                                                                    .with_fg(hud.text_muted),
                                                            );
                                                            let mut line = cast_session_label(
                                                                session.state,
                                                            )
                                                            .to_owned();
                                                            if session.frames_sent > 0 {
                                                                line.push_str(&format!(
                                                                    " · {} frames",
                                                                    session.frames_sent
                                                                ));
                                                            }
                                                            if let Some(audio) = &session.audio {
                                                                line.push_str(" · ");
                                                                line.push_str(audio);
                                                            }
                                                            if let Some(failure) =
                                                                &session.failure
                                                            {
                                                                line.push_str(" · ");
                                                                line.push_str(failure);
                                                            }
                                                            display_label(
                                                                f,
                                                                &line,
                                                                type_scale.footnote,
                                                            );
                                                        },
                                                    );

                                                    f.flex(1.0);
                                                    f.spacer(0.0);

                                                    if session.state
                                                        != tessera_desktop::system::CastSessionState::Failed
                                                    {
                                                        let (stop, _) = f.pressable_row(
                                                            &format!(
                                                                "tessera-cast-stop-{index}"
                                                            ),
                                                            "Stop",
                                                            &LayoutOpts {
                                                                width: 72.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                cross: Align::Center,
                                                                bg: hud.surface,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(
                                                                    themes::hud(&hud)
                                                                        .with_fg(hud.text),
                                                                );
                                                                display_label(
                                                                    f,
                                                                    "Stop",
                                                                    type_scale.caption,
                                                                );
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if stop.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::DisconnectCast {
                                                                    session: session.id,
                                                                },
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                            let _ = row;
                                        }
                                    }

                                    // Section: nearby sinks
                                    f.spacer(4.0);
                                    display_label(f, "Nearby Displays", type_scale.headline);

                                    if is_scanning {
                                        f.row_ex(
                                            &LayoutOpts {
                                                width: area.w,
                                                height: 40.0,
                                                pad: 8.0,
                                                radius: 10.0,
                                                bg: hud.surface_recessed,
                                                cross: Align::Center,
                                                gap: 8.0,
                                                ..Default::default()
                                            },
                                            |f| {
                                                f.icon_raw(
                                                    lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW,
                                                    16.0,
                                                );
                                                f.set_theme(
                                                    themes::hud(&hud).with_fg(hud.text_muted),
                                                );
                                                display_label(
                                                    f,
                                                    "Searching for nearby Wi-Fi Display (Miracast) sinks…",
                                                    type_scale.footnote,
                                                );
                                            },
                                        );
                                    }

                                    if status.cast.sinks.is_empty() {
                                        f.column_ex(
                                            &LayoutOpts {
                                                width: area.w,
                                                pad: 16.0,
                                                radius: 12.0,
                                                bg: hud.surface_recessed,
                                                cross: Align::Center,
                                                gap: 6.0,
                                                ..Default::default()
                                            },
                                            |f| {
                                                f.icon_raw(
                                                    lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                    24.0,
                                                );
                                                f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                display_label(
                                                    f,
                                                    "No wireless displays found",
                                                    type_scale.body,
                                                );
                                                f.set_theme(
                                                    themes::hud(&hud).with_fg(hud.text_muted),
                                                );
                                                display_label(
                                                    f,
                                                    "Ensure screen mirroring is enabled on your display, then click Scan.",
                                                    type_scale.footnote,
                                                );
                                            },
                                        );
                                    } else {
                                        for (index, sink) in status.cast.sinks.iter().enumerate() {
                                            let (row, _) = f.pressable_row(
                                                &format!("tessera-cast-device-{index}"),
                                                &sink.name,
                                                &LayoutOpts {
                                                    width: area.w,
                                                    height: 56.0,
                                                    pad: 8.0,
                                                    radius: 12.0,
                                                    bg: hud.surface_recessed,
                                                    cross: Align::Center,
                                                    gap: 10.0,
                                                    ..Default::default()
                                                },
                                                |f, _| {
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(hud.text),
                                                    );
                                                    f.icon_raw(
                                                        lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                        20.0,
                                                    );
                                                    f.column_ex(
                                                        &LayoutOpts {
                                                            cross: Align::Start,
                                                            gap: 1.0,
                                                            ..Default::default()
                                                        },
                                                        |f| {
                                                            display_label(
                                                                f,
                                                                &sink.name,
                                                                type_scale.body,
                                                            );
                                                            f.set_theme(
                                                                themes::hud(&hud)
                                                                    .with_fg(hud.text_muted),
                                                            );
                                                            let capabilities = sink
                                                                .capabilities
                                                                .iter()
                                                                .map(|capability| {
                                                                    cast_capability_label(*capability)
                                                                })
                                                                .collect::<Vec<_>>()
                                                                .join(" · ");
                                                            let line = if capabilities.is_empty() {
                                                                "Miracast (Wi-Fi Display)".to_owned()
                                                            } else {
                                                                format!(
                                                                    "Miracast (Wi-Fi Display) · {capabilities}"
                                                                )
                                                            };
                                                            display_label(
                                                                f,
                                                                &line,
                                                                type_scale.footnote,
                                                            );
                                                            if let Some(blocker) = &sink.blocker {
                                                                display_label(
                                                                    f,
                                                                    blocker,
                                                                    type_scale.footnote,
                                                                );
                                                            }
                                                        },
                                                    );

                                                    f.flex(1.0);
                                                    f.spacer(0.0);

                                                    if sink.blocker.is_none() && service_available {
                                                        let (connect, _) = f.pressable_row(
                                                            &format!(
                                                                "tessera-cast-connect-sink-{index}"
                                                            ),
                                                            "Connect",
                                                            &LayoutOpts {
                                                                width: 80.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                cross: Align::Center,
                                                                bg: hud.accent,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(
                                                                    themes::hud(&hud).with_fg(
                                                                        Color::rgba(
                                                                            255, 255, 255, 255,
                                                                        ),
                                                                    ),
                                                                );
                                                                display_label(
                                                                    f,
                                                                    "Connect",
                                                                    type_scale.caption,
                                                                );
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if connect.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::ConnectCast {
                                                                    sink: sink.id.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                            let _ = row;
                                        }
                                    }

                                    // Guidance
                                    f.row_ex(
                                        &LayoutOpts {
                                            width: area.w,
                                            pad: 12.0,
                                            radius: 12.0,
                                            bg: hud.surface_recessed,
                                            border: hud.border,
                                            border_width: 1.0,
                                            cross: Align::Start,
                                            gap: 10.0,
                                            ..Default::default()
                                        },
                                        |f| {
                                            f.icon_raw(
                                                lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                18.0,
                                            );
                                            f.column_ex(
                                                &LayoutOpts {
                                                    cross: Align::Start,
                                                    gap: 4.0,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(hud.text),
                                                    );
                                                    display_label(
                                                        f,
                                                        "How to connect to a wireless display",
                                                        type_scale.body,
                                                    );
                                                    f.set_theme(
                                                        themes::hud(&hud)
                                                            .with_fg(hud.text_muted),
                                                    );
                                                    display_label(
                                                        f,
                                                        "1. Enable 'Screen Mirroring' or 'Wireless Display' on your TV or projector.",
                                                        type_scale.footnote,
                                                    );
                                                    display_label(
                                                        f,
                                                        "2. Press Scan; the source advertises itself and the TV connects back over Wi-Fi Direct.",
                                                        type_scale.footnote,
                                                    );
                                                    display_label(
                                                        f,
                                                        "3. If the service reports a firewall error, allow inbound tcp/7236 on the P2P interface.",
                                                        type_scale.footnote,
                                                    );
                                                },
                                            );
                                        },
                                    );
                                },
                            );
                        });
                    },
                );
            },
        );
        f.set_theme(original);
    }
}

fn cast_session_label(state: tessera_desktop::system::CastSessionState) -> &'static str {
    use tessera_desktop::system::CastSessionState;
    match state {
        CastSessionState::Connecting => "Connecting",
        CastSessionState::Negotiating => "Negotiating",
        CastSessionState::Streaming => "Streaming",
        CastSessionState::Stopping => "Stopping",
        CastSessionState::Failed => "Failed",
    }
}

fn cast_capability_label(capability: tessera_desktop::system::CastCapability) -> &'static str {
    use tessera_desktop::system::CastCapability;
    match capability {
        CastCapability::ScreenMirroring => "Screen mirroring",
        CastCapability::Audio => "Audio",
        CastCapability::InputBackchannel => "Input",
        CastCapability::ContentProtection => "HDCP",
    }
}

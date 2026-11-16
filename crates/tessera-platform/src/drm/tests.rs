use super::*;

fn fake_overlay_plane(
    alpha: bool,
    zpos: bool,
    in_fence_fd: bool,
    damage_clips: bool,
) -> OverlayPlaneCapabilities {
    let raw = |value| std::num::NonZeroU32::new(value).expect("non-zero test handle");
    OverlayPlaneCapabilities {
        handle: plane::Handle::from(raw(30)),
        possible_crtcs: Vec::new(),
        formats: Vec::new(),
        zpos: zpos.then_some((1, 4)),
        alpha,
        in_fence_fd,
        fb_damage_clips: damage_clips,
        props: OverlayPlaneProperties {
            fb_id: property::Handle::from(raw(31)),
            crtc_id: property::Handle::from(raw(32)),
            src_x: property::Handle::from(raw(33)),
            src_y: property::Handle::from(raw(34)),
            src_w: property::Handle::from(raw(35)),
            src_h: property::Handle::from(raw(36)),
            crtc_x: property::Handle::from(raw(37)),
            crtc_y: property::Handle::from(raw(38)),
            crtc_w: property::Handle::from(raw(39)),
            crtc_h: property::Handle::from(raw(40)),
            alpha: alpha.then(|| property::Handle::from(raw(41))),
            zpos: zpos.then(|| property::Handle::from(raw(42))),
            in_fence_fd: in_fence_fd.then(|| property::Handle::from(raw(43))),
            fb_damage_clips: damage_clips.then(|| property::Handle::from(raw(44))),
        },
    }
}

#[test]
fn overlay_plane_request_carries_full_geometry_and_optional_properties() {
    let raw = |value| std::num::NonZeroU32::new(value).expect("non-zero test handle");
    let framebuffer = control::framebuffer::Handle::from(raw(20));
    let crtc = crtc::Handle::from(raw(21));
    let fence: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    let values = overlay_plane_request(
        &fake_overlay_plane(true, true, true, true),
        framebuffer,
        crtc,
        640,
        480,
        tessera_primitives::Rect::new(100, 50, 640, 480),
        Some(2),
        Some(&fence),
    )
    .unwrap();
    assert_eq!(values.framebuffer, framebuffer);
    assert_eq!(values.crtc, crtc);
    assert_eq!(values.src, (0, 0, 640 << 16, 480 << 16));
    assert_eq!(values.dst, (100, 50, 640, 480));
    assert_eq!(values.alpha, Some(0xffff));
    assert_eq!(values.zpos, Some(2));
    assert!(values.in_fence_fd.is_some());
    assert!(values.damage_clips);
}

#[test]
fn overlay_plane_request_rejects_unsupported_zpos_and_fence() {
    let raw = |value| std::num::NonZeroU32::new(value).expect("non-zero test handle");
    let framebuffer = control::framebuffer::Handle::from(raw(20));
    let crtc = crtc::Handle::from(raw(21));
    let fence: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    assert!(matches!(
        overlay_plane_request(
            &fake_overlay_plane(false, false, false, false),
            framebuffer,
            crtc,
            640,
            480,
            tessera_primitives::Rect::new(0, 0, 640, 480),
            Some(2),
            None,
        ),
        Err(DrmError::ScanoutUnsupported)
    ));
    assert!(matches!(
        overlay_plane_request(
            &fake_overlay_plane(false, false, false, false),
            framebuffer,
            crtc,
            640,
            480,
            tessera_primitives::Rect::new(0, 0, 640, 480),
            None,
            Some(&fence),
        ),
        Err(DrmError::ScanoutUnsupported)
    ));
    assert!(matches!(
        overlay_plane_request(
            &fake_overlay_plane(false, false, false, false),
            framebuffer,
            crtc,
            0,
            480,
            tessera_primitives::Rect::new(0, 0, 640, 480),
            None,
            None,
        ),
        Err(DrmError::ScanoutUnsupported)
    ));
}

#[test]
fn wheel_scaling_keeps_value120_and_steps_consistent() {
    // Neutral factor: one full detent unchanged.
    let neutral = scaled_wheel_value(120, 1.0);
    assert_eq!(neutral.value, Some(10.0));
    assert_eq!(neutral.discrete, Some(1));
    assert_eq!(neutral.value120, Some(120));
    // Double speed doubles every representation.
    let doubled = scaled_wheel_value(120, 2.0);
    assert_eq!(doubled.value, Some(20.0));
    assert_eq!(doubled.discrete, Some(2));
    assert_eq!(doubled.value120, Some(240));
    // Half a detent doubled becomes exactly one detent.
    let fraction = scaled_wheel_value(60, 2.0);
    assert_eq!(fraction.discrete, Some(1));
    assert_eq!(fraction.value120, Some(120));
    // A click slowed below one detent still steps once for legacy v5–7
    // clients while the continuous value carries the fraction.
    let slow = scaled_wheel_value(120, 0.5);
    assert_eq!(slow.value, Some(5.0));
    assert_eq!(slow.discrete, Some(1));
    assert_eq!(slow.value120, Some(60));
    // High-resolution fractions carry no whole step at neutral speed so
    // legacy clients do not see one full click per fraction.
    let fraction = scaled_wheel_value(30, 1.0);
    assert_eq!(fraction.discrete, None);
    assert_eq!(fraction.value120, Some(30));
    // A fraction scaled below the reporting floor (rounds to zero 120ths)
    // is suppressed entirely.
    assert_eq!(scaled_wheel_value(10, 0.04), PointerAxis::default());
    // A negative factor inverts direction (defensive; the UI range is
    // positive).
    let inverted = scaled_wheel_value(120, -1.0);
    assert_eq!(inverted.value, Some(-10.0));
    assert_eq!(inverted.discrete, Some(-1));
    assert_eq!(inverted.value120, Some(-120));
}

#[test]
fn test_inv_arch_122_primary_source_and_lifetime_are_one_contract() {
    assert!(
        PrimaryScanoutOwnership::TransientCompositor.matches_source(PrimaryPlaneSource::Composited)
    );
    assert!(
        !PrimaryScanoutOwnership::TransientCompositor
            .matches_source(PrimaryPlaneSource::DirectClient)
    );
    assert!(
        PrimaryScanoutOwnership::TransientClient.matches_source(PrimaryPlaneSource::DirectClient)
    );
    assert!(
        !PrimaryScanoutOwnership::TransientClient.matches_source(PrimaryPlaneSource::Composited)
    );
}

#[test]
fn test_inv_arch_135_overlay_requires_composited_primary() {
    assert!(overlay_source_supported(PrimaryPlaneSource::Composited));
    assert!(!overlay_source_supported(PrimaryPlaneSource::DirectClient));
}

/// [INV-ARCH-70]: a cached client direct-scanout import must satisfy the
/// same primary-plane source contract as the uncached transient form, so a
/// cache hit can never be paired with a composited frame (and vice versa).
#[test]
fn cached_client_ownership_is_direct_client_only() {
    let cached = PrimaryScanoutOwnership::CachedClient(ClientFbKey {
        identity: DmabufIdentity {
            device: 1,
            inode: 2,
        },
        width: 4,
        height: 4,
        stride: 16,
        offset: 0,
        fourcc: 0,
        modifier: 0,
    });
    assert!(cached.matches_source(PrimaryPlaneSource::DirectClient));
    assert!(!cached.matches_source(PrimaryPlaneSource::Composited));
}

#[test]
fn damage_clip_intersects_output_rect() {
    use tessera_primitives::Rect;
    // Unknown damage covers the whole output.
    assert_eq!(
        damage_clip_for_output(None, 1920, 0, 1920, 1080),
        vec![[1920, 0, 3840, 1080]]
    );
    // A box spanning both outputs of a side-by-side desktop clips to each.
    let damage_rects = [Rect::new(1900, 100, 100, 200)];
    let damage = Some(&damage_rects[..]);
    assert_eq!(
        damage_clip_for_output(damage, 0, 0, 1920, 1080),
        vec![[1900, 100, 1920, 300]]
    );
    assert_eq!(
        damage_clip_for_output(damage, 1920, 0, 1920, 1080),
        vec![[1920, 100, 2000, 300]]
    );
    // Disjoint damage leaves the output untouched.
    let other_rects = [Rect::new(0, 0, 100, 100)];
    let other = Some(&other_rects[..]);
    assert_eq!(
        damage_clip_for_output(other, 1920, 0, 1920, 1080),
        Vec::<[i32; 4]>::new()
    );
    // Damage fully outside the framebuffer on the negative side.
    let negative_rects = [Rect::new(-50, -50, 40, 40)];
    let negative = Some(&negative_rects[..]);
    assert_eq!(
        damage_clip_for_output(negative, 0, 0, 1920, 1080),
        Vec::<[i32; 4]>::new()
    );
    // Disjoint dirty regions are preserved as separate clips.
    let disjoint_rects = [Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)];
    let disjoint = Some(&disjoint_rects[..]);
    assert_eq!(
        damage_clip_for_output(disjoint, 0, 0, 1920, 1080),
        vec![[10, 10, 15, 15], [500, 400, 508, 408]]
    );
}

#[test]
fn cursor_plane_rect_clips_hotspot_and_output_boundaries() {
    assert_eq!(
        cursor_plane_rect((100, 80), (4, 6), (24, 24), (0, 0), (1920, 1080)),
        Some(CursorPlaneRect {
            src: (0, 0, 24, 24),
            dst: (96, 74, 24, 24),
        })
    );
    // Hotspot near the top-left keeps the full source and lets KMS clip a
    // negative destination, as cursor planes conventionally require.
    assert_eq!(
        cursor_plane_rect((2, 3), (6, 7), (24, 24), (0, 0), (1920, 1080)),
        Some(CursorPlaneRect {
            src: (0, 0, 24, 24),
            dst: (-4, -4, 24, 24),
        })
    );
    // The same sprite can be committed to two cursor planes while it
    // straddles side-by-side outputs.
    assert_eq!(
        cursor_plane_rect((1920, 100), (12, 12), (24, 24), (0, 0), (1920, 1080)),
        Some(CursorPlaneRect {
            src: (0, 0, 24, 24),
            dst: (1908, 88, 24, 24),
        })
    );
    assert_eq!(
        cursor_plane_rect((1920, 100), (12, 12), (24, 24), (1920, 0), (1920, 1080),),
        Some(CursorPlaneRect {
            src: (0, 0, 24, 24),
            dst: (-12, 88, 24, 24),
        })
    );
}

#[test]
fn poll_deadline_shapes_timeout_values() {
    // No deadline blocks indefinitely and never expires.
    assert_eq!(poll_ms_remaining(None), -1);
    assert!(!deadline_passed(None));

    // A future deadline yields a bounded positive remaining budget.
    let future = std::time::Instant::now() + Duration::from_secs(60);
    let remaining = poll_ms_remaining(Some(future));
    assert!((1..=60_000).contains(&remaining));
    assert!(!deadline_passed(Some(future)));

    // A passed deadline degrades to a non-blocking poll.
    let past = std::time::Instant::now() - Duration::from_secs(1);
    assert_eq!(poll_ms_remaining(Some(past)), 0);
    assert!(deadline_passed(Some(past)));
}

#[test]
fn busy_nonblocking_commit_is_a_transient_frame_skip() {
    let error = std::io::Error::from_raw_os_error(libc::EBUSY);
    assert!(commit_error_is_transient(&error));
    assert!(matches!(commit_error(error), DrmError::Busy));
}

#[test]
fn card_candidates_honor_explicit_override() {
    assert_eq!(
        candidate_cards_with_override(Some("/dev/dri/card-test".into())),
        vec![PathBuf::from("/dev/dri/card-test")]
    );
}

#[test]
fn imported_buffer_exposes_single_xrgb_plane() {
    let buffer = ImportedBuffer {
        size: (1920, 1080),
        stride: 7680,
        modifier: DrmModifier::Linear,
        format: DrmFourcc::Xrgb8888,
        offset: 0,
        gem: BufferHandle::from(std::num::NonZeroU32::new(1).unwrap()),
    };
    assert_eq!(buffer.size(), (1920, 1080));
    assert_eq!(buffer.format(), DrmFourcc::Xrgb8888);
    assert_eq!(buffer.pitches(), [7680, 0, 0, 0]);
    assert_eq!(buffer.offsets(), [0; 4]);
    assert_eq!(buffer.modifier(), Some(DrmModifier::Linear));
}

#[test]
fn parses_in_formats_modifier_bitsets() {
    // Header + two fourcc values + one aligned modifier record.
    let mut blob = vec![0_u8; 56];
    blob[8..12].copy_from_slice(&2_u32.to_ne_bytes());
    blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
    blob[16..20].copy_from_slice(&1_u32.to_ne_bytes());
    blob[20..24].copy_from_slice(&32_u32.to_ne_bytes());
    blob[24..28].copy_from_slice(&(DrmFourcc::Argb8888 as u32).to_ne_bytes());
    blob[28..32].copy_from_slice(&(DrmFourcc::Xrgb8888 as u32).to_ne_bytes());
    blob[32..40].copy_from_slice(&0b10_u64.to_ne_bytes());
    blob[40..44].copy_from_slice(&0_u32.to_ne_bytes());
    blob[48..56].copy_from_slice(&0x1234_u64.to_ne_bytes());

    assert_eq!(
        parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).unwrap(),
        vec![0x1234]
    );
    assert!(
        parse_format_modifiers(&blob, DrmFourcc::Argb8888 as u32)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn native_modifier_precedes_linear_fallback() {
    // Preserve deterministic preference even when the kernel blob lists
    // LINEAR first: output composition should use a GPU-native tiled
    // layout whenever the primary plane and Vulkan both accept it.
    let mut blob = vec![0_u8; 80];
    blob[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
    blob[16..20].copy_from_slice(&2_u32.to_ne_bytes());
    blob[20..24].copy_from_slice(&32_u32.to_ne_bytes());
    blob[24..28].copy_from_slice(&(DrmFourcc::Xrgb8888 as u32).to_ne_bytes());

    let linear = u64::from(DrmModifier::Linear);
    let tiled = u64::from(DrmModifier::I915_x_tiled);
    for (base, modifier) in [(32, linear), (56, tiled)] {
        blob[base..base + 8].copy_from_slice(&1_u64.to_ne_bytes());
        blob[base + 8..base + 12].copy_from_slice(&0_u32.to_ne_bytes());
        blob[base + 16..base + 24].copy_from_slice(&modifier.to_ne_bytes());
    }

    assert_eq!(
        parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).unwrap(),
        vec![tiled, linear]
    );
}

#[test]
fn rejects_out_of_bounds_in_formats_blob() {
    let mut blob = vec![0_u8; 24];
    blob[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
    blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
    assert!(parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).is_err());
}

#[test]
fn output_assignment_backtracks_and_intersects_modifiers() {
    let raw = |value| std::num::NonZeroU32::new(value).unwrap();
    // Mode is a transparent kernel modeinfo value and assignment never
    // reads it; all-zero modeinfo is valid test storage.
    let mode: Mode = unsafe { std::mem::zeroed() };
    let candidates = vec![
        OutputCandidate {
            connector: connector::Handle::from(raw(1)),
            name: "A".into(),
            mode,
            physical_size_mm: None,
            ppi: None,
            kind: OutputKind::External,
            scale: Scale::IDENTITY,
            choices: vec![
                OutputChoice {
                    crtc: crtc::Handle::from(raw(10)),
                    plane: plane::Handle::from(raw(20)),
                    modifiers: vec![0, 7],
                },
                OutputChoice {
                    crtc: crtc::Handle::from(raw(11)),
                    plane: plane::Handle::from(raw(21)),
                    modifiers: vec![0],
                },
            ],
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
        OutputCandidate {
            connector: connector::Handle::from(raw(2)),
            name: "B".into(),
            mode,
            physical_size_mm: None,
            ppi: None,
            kind: OutputKind::External,
            scale: Scale::IDENTITY,
            choices: vec![OutputChoice {
                crtc: crtc::Handle::from(raw(10)),
                plane: plane::Handle::from(raw(20)),
                modifiers: vec![0, 9],
            }],
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
    ];

    let (selected, modifiers) = assign_outputs(&candidates).unwrap();
    assert_eq!(selected[0].crtc, crtc::Handle::from(raw(11)));
    assert_eq!(selected[1].crtc, crtc::Handle::from(raw(10)));
    assert_eq!(modifiers, vec![0]);
}

#[test]
fn scanout_capabilities_are_checked_per_client_format() {
    let mut formats = HashMap::new();
    formats.insert(DrmFourcc::Xrgb8888 as u32, vec![0, 7]);
    formats.insert(DrmFourcc::Argb8888 as u32, vec![0, 9]);

    assert!(device::scanout_formats_support(
        &formats,
        DrmFourcc::Argb8888 as u32,
        9
    ));
    assert!(!device::scanout_formats_support(
        &formats,
        DrmFourcc::Argb8888 as u32,
        7
    ));
    assert!(!device::scanout_formats_support(
        &formats,
        DrmFourcc::Abgr8888 as u32,
        0
    ));
}

#[test]
fn scanout_modifier_intersection_requires_every_output() {
    assert_eq!(
        output::intersect_modifier_sets(&[vec![0, 7, 9], vec![0, 9], vec![9, 11]]),
        vec![9]
    );
    assert!(output::intersect_modifier_sets(&[]).is_empty());
}

#[test]
fn pick_mode_without_spec_uses_highest_pixel_count_then_refresh() {
    let modes = [
        (1920, 1080, 60_000, false),
        (2560, 1440, 60_000, true),
        (2560, 1440, 144_000, false),
        (3840, 2160, 60_000, false),
    ];
    assert_eq!(pick_mode(&modes, None), Some(3));

    let same_resolution = [(2560, 1440, 60_000, true), (2560, 1440, 144_000, false)];
    assert_eq!(pick_mode(&same_resolution, None), Some(1));
    assert_eq!(pick_mode(&[], None), None);
}

#[test]
fn pick_mode_with_spec_requires_exact_size() {
    let modes = [(1920, 1080, 60_000, false), (2560, 1440, 60_000, true)];
    let spec: ModeSpec = "1920x1080".parse().unwrap();
    assert_eq!(pick_mode(&modes, Some(&spec)), Some(0));
    // Nothing matches → None so the caller can fall back and warn.
    let missing: ModeSpec = "1280x720".parse().unwrap();
    assert_eq!(pick_mode(&modes, Some(&missing)), None);
}

#[test]
fn pick_mode_with_spec_uses_highest_refresh_then_tie_breakers() {
    let modes = [
        (1920, 1080, 60_000, false),
        (1920, 1080, 144_000, false),
        (1920, 1080, 75_000, true),
    ];
    let spec: ModeSpec = "1920x1080".parse().unwrap();
    assert_eq!(pick_mode(&modes, Some(&spec)), Some(1));
    // An exact refresh request selects only that rate (rounded).
    let hz: ModeSpec = "1920x1080@144".parse().unwrap();
    assert_eq!(pick_mode(&modes, Some(&hz)), Some(1));
    let odd: ModeSpec = "1920x1080@75".parse().unwrap();
    assert_eq!(pick_mode(&modes, Some(&odd)), Some(2));
}

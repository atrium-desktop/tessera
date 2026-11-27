//! Glyph registration against real, device-backed optics-ui contexts.
//!
//! Production contexts are always device-backed. A headless (device-less)
//! context is deliberately not used here: Optics v0.0.88 allocates the icon
//! slot table with unzeroed `malloc` on that path, so growing it past 16
//! slots corrupts the registry.

use optics_ui::Ui;
use optics_ui::sys::optics_ui_icon_id;
use tessera_design::icons::{self, Glyph};

fn device_ui(device: &optics_gfx::Device) -> Ui {
    // SAFETY: `device` is live and outlives the returned context in every
    // caller below; the pointer crosses between the ABI-identical gfx types.
    unsafe { Ui::with_device(device.as_raw().cast::<optics_ui::sys::optics_gfx_device>()) }
        .expect("device-backed optics-ui context")
}

#[test]
fn install_assigns_the_same_ids_in_every_fresh_context() {
    let Ok(device) = optics_gfx::Device::new(true, &[], &[], 0) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut first = device_ui(&device);
    icons::install(&mut first).expect("glyphs install into a fresh context");
    let wifi = Glyph::Wifi.id();
    assert_ne!(wifi, optics_ui_icon_id::OPTICS_UI_ICON_NONE);
    assert!(wifi.0 >= optics_ui_icon_id::OPTICS_UI_ICON_BUILTIN_COUNT.0);

    let mut second = device_ui(&device);
    icons::install(&mut second).expect("a second fresh context assigns identical ids");
    assert_eq!(Glyph::Wifi.id(), wifi);
    drop(second);
    drop(first);
}

#[test]
fn install_reports_a_context_that_already_registered_icons() {
    let Ok(device) = optics_gfx::Device::new(true, &[], &[], 0) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut reference = device_ui(&device);
    icons::install(&mut reference).expect("reference install");

    let mut dirty = device_ui(&device);
    let stray = [optics_ui::sys::optics_ui_icon_cmd {
        kind: optics_ui::sys::optics_ui_icon_cmd_kind::OPTICS_UI_ICON_CMD_CIRCLE,
        _pad: [0; 3],
        params: [12.0, 12.0, 4.0, 0.0, 0.0, 0.0],
    }];
    dirty
        .register_icon(
            &stray,
            &[],
            optics_ui::sys::optics_ui_icon_paint::OPTICS_UI_ICON_PAINT_STROKE,
        )
        .expect("stray icon");
    assert!(matches!(
        icons::install(&mut dirty),
        Err(icons::InstallError::Diverged { .. })
    ));
}

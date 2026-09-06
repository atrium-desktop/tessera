//! Chrome-host integration test: the HUD rendered through the real shell.
//!
//! Component crates test their own geometry headlessly; this test exercises
//! the full host path (lens context, registration, backdrop prepass, render)
//! and asserts the visible-pixels contract on the composited output.

use tessera_hud::Hud;
use tessera_model::workspace::{WorkspaceEntry, WorkspaceId, WorkspaceSnapshot};

fn workspaces_with(count: usize, current: usize) -> WorkspaceSnapshot {
    use tessera_model::workspace::{OutputId, OutputSnapshot};

    let entries = (0..count)
        .map(|index| WorkspaceEntry {
            id: WorkspaceId(index as u64),
            label: None,
            tiled: false,
            toplevels: Vec::new(),
        })
        .collect::<Vec<_>>();
    WorkspaceSnapshot {
        outputs: vec![OutputSnapshot {
            id: OutputId(0),
            connector: "nested".to_owned(),
            current: entries.get(current).map(|workspace| workspace.id),
            workspaces: entries,
        }],
    }
}

#[test]
fn hud_emits_visible_pixels_above_its_glass_body() {
    const WIDTH: usize = 800;
    const HEIGHT: usize = 80;
    let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
        return;
    };
    let Ok(surface) = flux::Surface::offscreen_readback(&device, WIDTH as u32, HEIGHT as u32)
    else {
        return;
    };
    let canvas = flux::Canvas::new(&surface).unwrap();
    let mut shell = unsafe { tessera_shell::Shell::new(device.as_raw().cast()) }.unwrap();
    shell.add(Box::new(Hud::new()));
    shell.set_workspaces(workspaces_with(1, 0));
    let mut input = lens::Input::new((WIDTH as f32, HEIGHT as f32), 1.0 / 60.0);
    input.set_cursor(10.0, 70.0);
    shell.prepare_backdrop(&input);

    let frame = surface.begin_frame().unwrap();
    canvas
        .begin_frame(Some(&frame), Some(flux::rgba(0, 0, 0, 255)))
        .unwrap();
    unsafe {
        shell
            .render(canvas.as_raw().cast(), &input)
            .expect("HUD must render into the offscreen canvas");
    }
    canvas.end_frame_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0; WIDTH * HEIGHT * 4];
    surface.read_pixels(&mut pixels).unwrap();
    let luminance = |x: usize, y: usize| {
        let offset = (y * WIDTH + x) * 4;
        u16::from(pixels[offset]) + u16::from(pixels[offset + 1]) + u16::from(pixels[offset + 2])
    };
    // Two slots make a 45 px chip centered at x=377.5. The active 7 px
    // sphere is centered at roughly (391, 24); x=380 is glass-only.
    assert!(
        luminance(391, 24) > luminance(380, 24) + 120,
        "the active workspace sphere must remain visible over its glass body",
    );
}

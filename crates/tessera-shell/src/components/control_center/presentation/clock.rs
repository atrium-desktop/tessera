//! The top-center clock surface and its locale wall-clock formatting.

use super::*;

impl ControlCenter {
    /// The frameless clock surface at top-center: large locale time with
    /// the weekday and date beneath. Redrawn when the wall clock's minute
    /// advances; between minutes the cached strings render unchanged.
    pub(crate) fn render_clock_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        if rect.w < 80.0 || rect.h < 40.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let fall = (1.0 - ease_out_cubic(progress)) * 16.0;
        let rect = Rect {
            y: rect.y + fall,
            ..rect
        };
        let (time_text, date_text) = clock_strings();

        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);

        f.set_theme(base_theme);
        let time_h = (rect.h * 0.62).max(24.0);
        f.place(
            "tessera-hud-clock-time",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: time_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, time_h, |f| {
                    display_label(f, &time_text, type_scale.hero);
                });
            },
        );
        f.set_theme(muted_theme);
        let date_h = (rect.h - time_h).max(16.0);
        f.place(
            "tessera-hud-clock-date",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y + time_h,
                    w: rect.w,
                    h: date_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, date_h, |f| {
                    display_label(f, &date_text, type_scale.headline);
                });
            },
        );
        f.set_theme(original);
    }
}

/// Locale wall-clock strings for the top-center surface: `("21:47",
/// "Saturday, June 21")`. libc `localtime_r` keeps this off any additional
/// dependency, matching the lock screen's clock.
pub(crate) fn clock_strings() -> (String, String) {
    use std::ffi::{CStr, c_char};

    let mut timestamp = 0;
    unsafe {
        libc::time(&mut timestamp);
    }
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let local = unsafe {
        if libc::localtime_r(&timestamp, local.as_mut_ptr()).is_null() {
            return ("--:--".into(), String::new());
        }
        local.assume_init()
    };
    let strftime = |time: &libc::tm, format: &CStr| -> String {
        let mut output = [0 as c_char; 128];
        let len =
            unsafe { libc::strftime(output.as_mut_ptr(), output.len(), format.as_ptr(), time) };
        if len == 0 {
            String::new()
        } else {
            unsafe { CStr::from_ptr(output.as_ptr()) }
                .to_string_lossy()
                .trim()
                .to_owned()
        }
    };
    (strftime(&local, c"%H:%M"), strftime(&local, c"%A, %B %e"))
}

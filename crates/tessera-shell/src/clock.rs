//! The one wall-clock formatter for shell surfaces.
//!
//! The dock's clock cell and the command panel's clock both read it. Two copies
//! of a locale time format is two chances for the same session to show two
//! different times, which is exactly the kind of disagreement a user notices.

pub fn clock_strings() -> (String, String) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_strings_render_a_time_and_a_date() {
        let (time, date) = clock_strings();
        assert!(
            time.contains(':'),
            "wall-clock time renders as HH:MM: {time}"
        );
        assert!(!date.is_empty(), "the date line renders: {date}");
    }
}

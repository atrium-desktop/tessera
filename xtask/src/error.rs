//! Minimal error/context helpers replacing `anyhow` in `xtask`.

use std::fmt::Display;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub trait Context<T> {
    fn context<C: Display>(self, context: C) -> Result<T>;
    fn with_context<C: Display, F: FnOnce() -> C>(self, f: F) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> Context<T> for std::result::Result<T, E> {
    fn context<C: Display>(self, context: C) -> Result<T> {
        self.map_err(|err| format!("{context}: {err}").into())
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.map_err(|err| format!("{}: {err}", f()).into())
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: Display>(self, context: C) -> Result<T> {
        self.ok_or_else(|| format!("{context}").into())
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.ok_or_else(|| format!("{}", f()).into())
    }
}

macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(format!($($arg)*).into())
    };
}

pub(crate) use bail;

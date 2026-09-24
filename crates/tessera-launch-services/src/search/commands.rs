//! System command palette definitions and matching for the Intent engine.

use tessera_desktop::app::BuiltInApplication;
use tessera_desktop::system::SystemAction;
use tessera_i18n::{Localizer, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemCmd {
    Lock,
    Screenshot,
    Suspend,
    Restart,
    PowerOff,
}

impl SystemCmd {
    pub const ALL: [SystemCmd; 5] = [
        SystemCmd::Lock,
        SystemCmd::Screenshot,
        SystemCmd::Suspend,
        SystemCmd::Restart,
        SystemCmd::PowerOff,
    ];

    #[must_use]
    pub fn matches(self, query: &str) -> bool {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return false;
        }

        match self {
            Self::Lock => {
                "lock".starts_with(&q) || "lockscreen".starts_with(&q) || query.contains("锁屏")
            }
            Self::Screenshot => {
                "screenshot".starts_with(&q)
                    || "screen".starts_with(&q)
                    || "prtsc".starts_with(&q)
                    || query.contains("截图")
                    || query.contains("截屏")
            }
            Self::Suspend => {
                "suspend".starts_with(&q)
                    || "sleep".starts_with(&q)
                    || query.contains("睡眠")
                    || query.contains("休眠")
            }
            Self::Restart => {
                "restart".starts_with(&q) || "reboot".starts_with(&q) || query.contains("重启")
            }
            Self::PowerOff => {
                "poweroff".starts_with(&q)
                    || "shutdown".starts_with(&q)
                    || query.contains("关机")
                    || query.contains("电源")
            }
        }
    }

    #[must_use]
    pub fn title(self, i18n: &Localizer) -> &'static str {
        match self {
            Self::Lock => i18n.text(Message::LockNow),
            Self::Screenshot => "Screenshot",
            Self::Suspend => i18n.text(Message::Suspend),
            Self::Restart => i18n.text(Message::Restart),
            Self::PowerOff => i18n.text(Message::PowerOff),
        }
    }

    #[must_use]
    pub fn subtitle(self, _i18n: &Localizer) -> &'static str {
        match self {
            Self::Lock => "System · Secure current session",
            Self::Screenshot => "System · Capture screen region",
            Self::Suspend => "System · Put machine to sleep",
            Self::Restart => "System · Reboot operating system",
            Self::PowerOff => "System · Shut down computer",
        }
    }

    /// Convert system command into corresponding domain system action, if applicable.
    #[must_use]
    pub fn to_system_action(self) -> Option<SystemAction> {
        match self {
            Self::Suspend => Some(SystemAction::Suspend),
            Self::Restart => Some(SystemAction::Reboot),
            Self::PowerOff => Some(SystemAction::PowerOff),
            Self::Lock | Self::Screenshot => None,
        }
    }

    /// Convert system command into corresponding built-in application, if applicable.
    #[must_use]
    pub fn to_builtin_app(self) -> Option<BuiltInApplication> {
        match self {
            Self::Screenshot => Some(BuiltInApplication::ScreenshotSelector),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_keyword_matching() {
        assert!(SystemCmd::Lock.matches("loc"));
        assert!(SystemCmd::Lock.matches("锁屏"));
        assert!(!SystemCmd::Lock.matches("sleep"));

        assert!(SystemCmd::Screenshot.matches("screen"));
        assert!(SystemCmd::Screenshot.matches("截图"));

        assert!(SystemCmd::Suspend.matches("sleep"));
        assert!(SystemCmd::Suspend.matches("休眠"));

        assert!(SystemCmd::Restart.matches("reboot"));
        assert!(SystemCmd::Restart.matches("重启"));

        assert!(SystemCmd::PowerOff.matches("shutdown"));
        assert!(SystemCmd::PowerOff.matches("关机"));
    }
}

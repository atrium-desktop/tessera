//! System command palette definitions and matching for Pivot.

use lens::Icon;
use tessera_chrome::ChromeEvents;
use tessera_i18n::{Localizer, Message};
use tessera_model::app::BuiltInApplication;
use tessera_model::system::SystemAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    #[must_use]
    pub fn icon(self) -> Icon {
        match self {
            Self::Lock => Icon::Shield,
            Self::Screenshot => Icon::Zap,
            Self::Suspend => Icon::Clock,
            Self::Restart => Icon::RotateCw,
            Self::PowerOff => Icon::X,
        }
    }

    pub fn execute(self, out: &mut ChromeEvents) {
        match self {
            Self::Lock => {
                out.lock = true;
            }
            Self::Screenshot => {
                out.open_builtin = Some(BuiltInApplication::ScreenshotSelector);
            }
            Self::Suspend => {
                out.system_actions.push(SystemAction::Suspend);
            }
            Self::Restart => {
                out.system_actions.push(SystemAction::Reboot);
            }
            Self::PowerOff => {
                out.system_actions.push(SystemAction::PowerOff);
            }
        }
    }
}

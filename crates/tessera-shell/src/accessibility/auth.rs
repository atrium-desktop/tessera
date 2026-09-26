//! Kernel peer authentication for accessibility consumers (`[INV-A11Y-04]`).

use std::path::{Path, PathBuf};

/// Policy deciding whether a calling process is authorized to inspect or interact
/// with the desktop shell's accessibility tree.
#[derive(Debug, Clone)]
pub struct PeerAuthenticator {
    /// Explicitly allowed executable paths (e.g. `/usr/bin/orca`).
    trusted_binaries: Vec<PathBuf>,
    /// Disallowed cgroup tokens indicating sandboxed/untrusted execution (e.g. `agent-domain-`).
    forbidden_cgroup_tokens: Vec<String>,
}

impl Default for PeerAuthenticator {
    fn default() -> Self {
        Self {
            trusted_binaries: vec![
                PathBuf::from("/usr/bin/orca"),
                PathBuf::from("/usr/local/bin/orca"),
                PathBuf::from("/usr/bin/squeekboard"),
                PathBuf::from("/usr/bin/maliit-keyboard"),
            ],
            forbidden_cgroup_tokens: vec![
                "agent-domain-".to_string(),
                "sandbox-".to_string(),
                "flatpak".to_string(),
            ],
        }
    }
}

impl PeerAuthenticator {
    /// Create a peer authenticator with custom white/black lists (useful for testing).
    #[must_use]
    pub fn new(trusted_binaries: Vec<PathBuf>, forbidden_cgroup_tokens: Vec<String>) -> Self {
        Self {
            trusted_binaries,
            forbidden_cgroup_tokens,
        }
    }

    /// Check whether a process with the given PID is authorized to access accessibility trees.
    #[must_use]
    pub fn is_authorized_pid(&self, pid: u32) -> bool {
        // 1. Resolve executable path via /proc/<pid>/exe
        let exe_link = format!("/proc/{pid}/exe");
        let Ok(exe_path) = std::fs::read_link(&exe_link) else {
            return false;
        };

        self.is_authorized_exe(&exe_path) && !self.is_sandboxed_pid(pid)
    }

    /// Check if the executable path matches a trusted binary.
    #[must_use]
    pub fn is_authorized_exe(&self, exe: &Path) -> bool {
        // Direct match with trusted binaries
        if self.trusted_binaries.iter().any(|b| b == exe) {
            return true;
        }

        // Match based on binary name if it ends with known screen reader / OSK
        if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
            if name == "orca" || name == "squeekboard" || name == "maliit-keyboard" {
                return true;
            }
        }

        false
    }

    /// Check if the cgroup for this process contains any forbidden isolation tokens.
    #[must_use]
    pub fn is_sandboxed_pid(&self, pid: u32) -> bool {
        let cgroup_path = format!("/proc/{pid}/cgroup");
        let Ok(cgroup_content) = std::fs::read_to_string(&cgroup_path) else {
            return false;
        };

        self.is_sandboxed_cgroup(&cgroup_content)
    }

    /// Check cgroup string against forbidden tokens.
    #[must_use]
    pub fn is_sandboxed_cgroup(&self, cgroup_content: &str) -> bool {
        for token in &self.forbidden_cgroup_tokens {
            if cgroup_content.contains(token) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_binary_is_accepted() {
        let auth = PeerAuthenticator::default();
        assert!(auth.is_authorized_exe(Path::new("/usr/bin/orca")));
        assert!(auth.is_authorized_exe(Path::new("/usr/local/bin/orca")));
        assert!(auth.is_authorized_exe(Path::new("/usr/bin/squeekboard")));
        assert!(!auth.is_authorized_exe(Path::new("/tmp/evil_script")));
        assert!(!auth.is_authorized_exe(Path::new("/usr/bin/python3")));
    }

    #[test]
    fn sandboxed_cgroup_is_rejected() {
        let auth = PeerAuthenticator::default();
        let safe_cgroup = "0::/user.slice/user-1000.slice/session-2.scope";
        let agent_cgroup = "0::/user.slice/user-1000.slice/agent-domain-42.scope";
        let flatpak_cgroup = "0::/user.slice/user-1000.slice/app-flatpak-org.example.app-123.scope";

        assert!(!auth.is_sandboxed_cgroup(safe_cgroup));
        assert!(auth.is_sandboxed_cgroup(agent_cgroup));
        assert!(auth.is_sandboxed_cgroup(flatpak_cgroup));
    }
}

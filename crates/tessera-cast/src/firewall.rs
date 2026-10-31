//! Inbound-firewall observation.
//!
//! Miracast makes the **source** the RTSP server: the sink connects to the
//! source's control port (7236 by default). A host firewall whose input policy
//! is `drop` silently eats that connection, and the failure looks exactly like
//! "the sink never answered" or "the sink is not a Miracast sink". That
//! misdiagnosis already cost a hardware session, so the probe reports the
//! firewall as its own fact (`[INV-ARCH-83]`: a definite blocker must be named,
//! never folded into "unverified").
//!
//! The observation is read-only: it reads the live ruleset when the process may,
//! and otherwise reports **unverified** with the persistent configuration as a
//! hint. A persistent file is never treated as the running state — doing that
//! produced a false "blocked" verdict on a machine whose live ruleset allowed
//! the sink to connect.

use std::path::Path;

/// The default WFD session-management (RTSP) port. The source listens here; the
/// sink connects to it.
pub const WFD_CONTROL_PORT: u16 = 7236;

/// Whether an inbound TCP connection to the WFD control port would be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundVerdict {
    /// An explicit rule accepts the port.
    Allowed(String),
    /// The input policy drops the port and no rule accepts it.
    Blocked(String),
    /// The firewall could not be inspected.
    Unverified(String),
}

impl InboundVerdict {
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked(_))
    }

    /// A one-line description with the evidence that produced it.
    pub fn describe(&self) -> String {
        match self {
            Self::Allowed(evidence) => format!("accepted ({evidence})"),
            Self::Blocked(evidence) => format!("BLOCKED ({evidence})"),
            Self::Unverified(reason) => format!("unverified ({reason})"),
        }
    }
}

/// Observe whether inbound TCP to `port` is accepted.
///
/// **Only a readable live ruleset is authoritative.** The persistent
/// `/etc/nftables.conf` is a hint, not the running state: it may not have been
/// loaded, or a runtime rule may have changed it. Reading the file and calling
/// it "blocked" produced a false blocker on the reference machine, where the TV
/// connected successfully while the file still said `policy drop`.
pub fn observe(port: u16) -> InboundVerdict {
    let live = std::process::Command::new("nft")
        .args(["list", "ruleset"])
        .output();
    if let Ok(output) = live
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() {
            return InboundVerdict::Allowed("the live nftables ruleset is empty".to_owned());
        }
        return classify_nftables(&text, port);
    }

    // The live ruleset could not be read (usually a privilege wall). Persistent
    // files are not evidence of the running state, so the result stays
    // unverified rather than borrowing their verdict.
    let mut nft_text = String::new();
    read_into(Path::new("/etc/nftables.conf"), &mut nft_text);
    collect_dir(Path::new("/etc/nftables.d"), &mut nft_text);
    if !nft_text.trim().is_empty() {
        return InboundVerdict::Unverified(
            "the live nftables ruleset could not be read; persistent configuration is not evidence of the live state"
                .to_owned(),
        );
    }

    let mut ufw_text = String::new();
    read_into(Path::new("/etc/ufw/user.rules"), &mut ufw_text);
    if !ufw_text.trim().is_empty() {
        let enabled = std::fs::read_to_string("/etc/ufw/ufw.conf")
            .map(|text| text.contains("ENABLED=yes"))
            .unwrap_or(false);
        let hint = classify_ufw(&ufw_text, port);
        if enabled {
            return hint;
        }
        return InboundVerdict::Unverified(
            "ufw is not enabled; its persistent rules are not evidence of the live state"
                .to_owned(),
        );
    }

    InboundVerdict::Unverified(
        "the live nftables ruleset could not be read and no ufw rules are configured".to_owned(),
    )
}

fn read_into(path: &Path, out: &mut String) {
    if let Ok(text) = std::fs::read_to_string(path) {
        out.push_str(&text);
        out.push('\n');
    }
}

fn collect_dir(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "conf" || extension == "nft")
        })
        .collect();
    paths.sort();
    for path in paths {
        read_into(&path, out);
    }
}

/// Classify an nftables ruleset for inbound TCP `port`.
pub fn classify_nftables(text: &str, port: u16) -> InboundVerdict {
    let Some(chain) = extract_input_chain(text) else {
        return InboundVerdict::Unverified("no `chain input` block in the ruleset".to_owned());
    };
    let policy = chain
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("policy "))
        .map(|value| value.trim().to_owned());
    let allowed = chain.lines().any(|line| accepts_port(line, port));
    match policy.as_deref() {
        Some("accept") => InboundVerdict::Allowed("input policy accept".to_owned()),
        Some("drop") if allowed => {
            InboundVerdict::Allowed(format!("an input rule accepts tcp/{port}"))
        }
        Some("drop") => {
            InboundVerdict::Blocked(format!("input policy drop and no rule accepts tcp/{port}"))
        }
        Some(other) => InboundVerdict::Unverified(format!("input policy `{other}`")),
        None => {
            if allowed {
                InboundVerdict::Allowed(format!("an input rule accepts tcp/{port}"))
            } else {
                InboundVerdict::Unverified("input chain has no policy".to_owned())
            }
        }
    }
}

/// Extract the body of `chain input { ... }`, balancing braces.
fn extract_input_chain(text: &str) -> Option<String> {
    let start = text.find("chain input")?;
    let open = text[start..].find('{')? + start;
    let mut depth = 0usize;
    let mut end = None;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    Some(text[open + 1..end].to_owned())
}

fn accepts_port(line: &str, port: u16) -> bool {
    let compact: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if !compact.contains("accept") {
        return false;
    }
    // A scoped interface rule is what a caster should ask an operator to add;
    // it is also what makes an otherwise-dropping ruleset usable for P2P.
    if compact.contains("iifname") || compact.contains("iif ") {
        let scoped = compact.contains("wlan") || compact.contains("p2p");
        if scoped {
            return true;
        }
    }
    let exact = format!("tcp dport {port}");
    if compact.contains(&exact) {
        return true;
    }
    // A set form: `tcp dport { 22, 7236 } accept`.
    if let Some(set_start) = compact.find("tcp dport {")
        && let Some(set_end_rel) = compact[set_start..].find('}')
    {
        let set = &compact[set_start + "tcp dport {".len()..set_start + set_end_rel];
        return set.split(',').any(|item| item.trim() == port.to_string());
    }
    false
}

/// Classify the persistent ufw rules for inbound TCP `port`.
fn classify_ufw(text: &str, port: u16) -> InboundVerdict {
    let accepts = text.lines().any(|line| {
        line.contains("-j ACCEPT")
            && line.contains("-p tcp")
            && (line.contains(&format!("--dport {port}"))
                || line.contains(&format!("--dport={port}")))
    });
    if accepts {
        return InboundVerdict::Allowed(format!("a ufw rule accepts tcp/{port}"));
    }
    let default_drop = std::fs::read_to_string("/etc/default/ufw")
        .map(|text| text.contains("DEFAULT_INPUT_POLICY=\"DROP\""))
        .unwrap_or(false);
    if default_drop {
        InboundVerdict::Blocked(format!(
            "ufw default input policy is DROP and no rule accepts tcp/{port}"
        ))
    } else {
        InboundVerdict::Unverified("ufw rules present but no matching rule".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = r#"
#!/usr/bin/nft -f

flush ruleset

table inet filter {
    chain input {
        type filter hook input priority filter; policy drop;
        iif lo accept
        ct state established,related accept
        ct state invalid drop
        tcp dport 22 accept
    }
    chain forward {
        type filter hook forward priority filter; policy drop;
    }
    chain output {
        type filter hook output priority filter; policy accept;
    }
}
"#;

    #[test]
    fn the_reference_ruleset_blocks_the_wfd_control_port() {
        let verdict = classify_nftables(REFERENCE, 7236);
        assert!(verdict.is_blocked(), "{verdict:?}");
        let described = verdict.describe();
        assert!(described.contains("policy drop"), "{described}");
        assert!(described.contains("7236"), "{described}");
    }

    #[test]
    fn an_explicit_port_accept_makes_it_allowed() {
        let text = REFERENCE.replace(
            "tcp dport 22 accept",
            "tcp dport 22 accept\n        tcp dport 7236 accept",
        );
        assert!(matches!(
            classify_nftables(&text, 7236),
            InboundVerdict::Allowed(_)
        ));
    }

    #[test]
    fn a_set_form_accept_is_understood() {
        let text = REFERENCE.replace("tcp dport 22 accept", "tcp dport { 22, 7236 } accept");
        assert!(matches!(
            classify_nftables(&text, 7236),
            InboundVerdict::Allowed(_)
        ));
    }

    #[test]
    fn a_scoped_p2p_interface_accept_is_understood() {
        let text = REFERENCE.replace(
            "tcp dport 22 accept",
            "tcp dport 22 accept\n        iifname \"wlan0-p2p*\" accept",
        );
        assert!(matches!(
            classify_nftables(&text, 7236),
            InboundVerdict::Allowed(_)
        ));
    }

    #[test]
    fn an_accept_policy_is_allowed() {
        let text = REFERENCE.replace(
            "policy drop;\n        iif lo",
            "policy accept;\n        iif lo",
        );
        assert!(matches!(
            classify_nftables(&text, 7236),
            InboundVerdict::Allowed(_)
        ));
    }

    #[test]
    fn an_unrecognised_ruleset_is_unverified_not_allowed() {
        assert!(matches!(
            classify_nftables(
                "table inet filter { chain output { policy accept; } }",
                7236
            ),
            InboundVerdict::Unverified(_)
        ));
        assert!(matches!(
            classify_nftables("", 7236),
            InboundVerdict::Unverified(_)
        ));
    }

    #[test]
    fn an_established_related_rule_is_not_a_new_inbound_accept() {
        // The sink's SYN is a new connection; conntrack does not make it
        // established, so this rule must not be read as an accept.
        let verdict = classify_nftables(REFERENCE, 7236);
        assert!(verdict.is_blocked(), "{verdict:?}");
    }
}

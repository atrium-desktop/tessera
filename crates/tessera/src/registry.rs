//! Agent principal registry: pairing, credentials, and approved capability
//! ceilings for capability-borrowing agents (ADR-0088).
//!
//! The registry is the compositor-held source of truth for "who may borrow
//! what". It lives in `$XDG_DATA_HOME/tessera/principals.json` with owner-only
//! permissions; credentials are stored as SHA-256 digests so the file alone
//! cannot be replayed. Durable runtime-grant decisions live alongside in
//! `grants.json` under the same discipline (see [`GrantStore`]).

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use tessera_authority::authority::{AgentIdentity, PairedAgent};
use tessera_protocol::ActorCapability;

const MAX_STATE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PRINCIPALS: usize = 4_096;
const MAX_GRANTS: usize = 16_384;

/// Operation families a self-registered agent may request. Component-only
/// operations (target/application picks, whole-output capture and streaming,
/// wallpaper, idle inhibition, global input, session control, plain
/// screenshots) are never
/// agent-requestable: they belong to platform components, not to borrowing
/// agents. Per-window content capture is the deliberate exception: unlike
/// whole-output capture it is bounded to one window the scope's `windows`
/// axis can name, and first use always routes through the interactive
/// runtime grant, so an agent may request it.
pub const AGENT_REQUESTABLE: &[ActorCapability] = &[
    ActorCapability::ObserveWindows,
    ActorCapability::ObserveWorkspaces,
    ActorCapability::ObserveOutputs,
    ActorCapability::ObserveNotifications,
    ActorCapability::ObserveJournal,
    ActorCapability::ObserveInteractionDomains,
    ActorCapability::ObserveSettings,
    ActorCapability::ObserveSystem,
    ActorCapability::Focus,
    ActorCapability::Minimize,
    ActorCapability::Close,
    ActorCapability::Move,
    ActorCapability::SetWindowGeometry,
    ActorCapability::Cycle,
    ActorCapability::SwitchWorkspace,
    ActorCapability::SwitchWorkspaceTo,
    ActorCapability::MoveToWorkspace,
    ActorCapability::ToggleOverview,
    ActorCapability::Notify,
    ActorCapability::DismissNotification,
    ActorCapability::ReadFile,
    ActorCapability::WriteFile,
    ActorCapability::AccessNetworkOrigin,
    ActorCapability::PromptSecret,
    ActorCapability::RequestPayment,
    ActorCapability::InjectInteractionDomainInput,
    ActorCapability::CreateInteractionDomain,
    ActorCapability::TransactInteractionDomain,
    ActorCapability::RevokeInteractionDomain,
    ActorCapability::CaptureInteractionDomain,
    ActorCapability::CaptureWindow,
    ActorCapability::ObserveInteractionDomain,
    ActorCapability::LaunchInInteractionDomain,
    ActorCapability::LaunchApp,
];

/// Operation families that always route through the interactive runtime
/// grant on first use, however the ceiling was approved: destructive,
/// privacy-sensitive, or authority-transferring (ADR-0088).
pub fn is_runtime_gated(op: ActorCapability) -> bool {
    matches!(
        op,
        ActorCapability::Close
            | ActorCapability::ReadFile
            | ActorCapability::WriteFile
            | ActorCapability::AccessNetworkOrigin
            | ActorCapability::PromptSecret
            | ActorCapability::RequestPayment
            | ActorCapability::InjectInteractionDomainInput
            | ActorCapability::CreateInteractionDomain
            | ActorCapability::TransactInteractionDomain
            | ActorCapability::RevokeInteractionDomain
            | ActorCapability::CaptureInteractionDomain
            | ActorCapability::CaptureWindow
            | ActorCapability::ObserveInteractionDomain
            | ActorCapability::LaunchInInteractionDomain
            | ActorCapability::LaunchApp
    )
}

const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RegistryFile {
    version: u32,
    #[serde(default)]
    principals: Vec<PrincipalRecord>,
}

/// One paired agent principal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrincipalRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub credential_sha256: String,
    pub pregranted: Vec<ActorCapability>,
    pub gated: Vec<ActorCapability>,
    pub created_at: u64,
}

/// The compositor-held principal registry (ADR-0088).
pub struct PrincipalRegistry {
    path: Option<PathBuf>,
    principals: Vec<PrincipalRecord>,
    /// First-party component identities valid only for this compositor
    /// process. They participate in normal credential lookup and live
    /// ceiling refresh but are never serialized to the durable registry.
    ephemeral: Vec<PrincipalRecord>,
    /// Lowercased labels whose pairing was denied this session; repeat
    /// requests are refused without prompting again.
    denied: HashSet<String>,
}

impl PrincipalRegistry {
    /// Load the registry from `path`. A missing file starts empty; a
    /// corrupt or version-mismatched file also starts empty (fail-closed)
    /// and is logged — never silently trusted.
    pub fn load(path: PathBuf) -> Self {
        let principals = match read_private_state(&path) {
            Ok(Some(bytes)) => match serde_json::from_slice::<RegistryFile>(&bytes) {
                Ok(file) if file.version == REGISTRY_VERSION && valid_registry_file(&file) => {
                    file.principals
                }
                Ok(file) => {
                    log::warn!(
                        "agent registry {}: unsupported or invalid version {}, starting empty",
                        path.display(),
                        file.version
                    );
                    Vec::new()
                }
                Err(error) => {
                    log::warn!(
                        "agent registry {}: unreadable ({error}), starting empty",
                        path.display()
                    );
                    Vec::new()
                }
            },
            Ok(None) => Vec::new(),
            Err(error) => {
                log::warn!(
                    "agent registry {}: unsafe or unreadable ({error}), starting empty",
                    path.display()
                );
                Vec::new()
            }
        };
        Self {
            path: Some(path),
            principals,
            ephemeral: Vec::new(),
            denied: HashSet::new(),
        }
    }

    /// A session-only registry for sessions without a durable data
    /// directory. Pairings then live only until the compositor exits.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            principals: Vec::new(),
            ephemeral: Vec::new(),
            denied: HashSet::new(),
        }
    }

    /// Recognize a credential, returning the bound principal identity.
    pub fn lookup(&self, credential: &str) -> Option<AgentIdentity> {
        let digest = sha256_hex(credential.as_bytes());
        self.principals
            .iter()
            .chain(&self.ephemeral)
            .find(|record| constant_time_eq(record.credential_sha256.as_bytes(), digest.as_bytes()))
            .and_then(|record| {
                tessera_authority::authority::ActorPrincipal::new(record.id.clone())
                    .ok()
                    .map(|principal| AgentIdentity {
                        principal,
                        pregranted: record.pregranted.clone(),
                        gated: record.gated.clone(),
                    })
            })
    }

    /// Resolve the live ceiling by authenticated principal id. Unlike the
    /// handshake credential lookup, this is used to reauthorize an already
    /// connected client after ceiling changes or principal revocation.
    pub fn identity_for_principal(&self, principal: &str) -> Option<AgentIdentity> {
        self.principals
            .iter()
            .chain(&self.ephemeral)
            .find(|record| record.id == principal)
            .and_then(|record| {
                tessera_authority::authority::ActorPrincipal::new(record.id.clone())
                    .ok()
                    .map(|principal| AgentIdentity {
                        principal,
                        pregranted: record.pregranted.clone(),
                        gated: record.gated.clone(),
                    })
            })
    }

    /// Whether another principal already carries this display label. Used
    /// by the pairing prompt to warn about look-alike installations
    /// (ADR-0088 TOFU continuity).
    pub fn label_collision(&self, label: &str) -> bool {
        self.principals.iter().chain(&self.ephemeral).any(|record| {
            record
                .label
                .as_deref()
                .is_some_and(|existing| existing.eq_ignore_ascii_case(label))
        })
    }

    /// Whether pairing this label was already denied this session.
    pub fn is_denied(&self, label: Option<&str>) -> bool {
        self.denied.contains(&deny_key(label))
    }

    /// Cache a pairing denial for the rest of the session.
    pub fn deny(&mut self, label: Option<&str>) {
        self.denied.insert(deny_key(label));
    }

    /// Issue a new principal for an approved pairing: the requested set is
    /// filtered down to the agent-requestable operations and split into
    /// pregranted and runtime-gated families. The credential is returned
    /// once; the registry keeps only its digest.
    pub fn issue(
        &mut self,
        label: Option<&str>,
        requested: &[ActorCapability],
    ) -> Result<PairedAgent, String> {
        let mut pregranted = Vec::new();
        let mut gated = Vec::new();
        for op in requested {
            if !AGENT_REQUESTABLE.contains(op) || pregranted.contains(op) || gated.contains(op) {
                continue;
            }
            if is_runtime_gated(*op) {
                gated.push(*op);
            } else {
                pregranted.push(*op);
            }
        }
        let (principal, credential) =
            self.register_record(label, pregranted.clone(), gated.clone())?;
        Ok(PairedAgent {
            principal: tessera_authority::authority::ActorPrincipal::new(principal)
                .map_err(str::to_owned)?,
            credential,
            pregranted,
            gated,
        })
    }

    /// Every registered principal, in registration order.
    pub fn principals(&self) -> &[PrincipalRecord] {
        &self.principals
    }

    /// Rename a principal's display label (`None` clears it). Unknown id
    /// errors.
    pub fn rename(&mut self, principal: &str, label: Option<&str>) -> Result<(), String> {
        validate_label(label)?;
        let Some(record) = self
            .principals
            .iter_mut()
            .find(|record| record.id == principal)
        else {
            return Err(format!("unknown agent principal {principal}"));
        };
        let previous = record.label.clone();
        record.label = label.map(str::to_owned);
        if let Err(error) = self.save() {
            self.principals
                .iter_mut()
                .find(|record| record.id == principal)
                .expect("renamed principal disappeared")
                .label = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Forget a principal: the record and its credential die. Unknown id
    /// errors.
    pub fn forget(&mut self, principal: &str) -> Result<(), String> {
        let Some(index) = self
            .principals
            .iter()
            .position(|record| record.id == principal)
        else {
            return Err(format!("unknown agent principal {principal}"));
        };
        let removed = self.principals.remove(index);
        if let Err(error) = self.save() {
            self.principals.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Replace a principal's approved ceiling. Both groups are filtered
    /// down to the agent-requestable operations and deduplicated, order
    /// preserved. Unknown id errors.
    pub fn set_ceiling(
        &mut self,
        principal: &str,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    ) -> Result<(), String> {
        let (pregranted, gated) = validate_explicit_ceiling(pregranted, gated)?;
        let Some(index) = self
            .principals
            .iter()
            .position(|record| record.id == principal)
        else {
            return Err(format!("unknown agent principal {principal}"));
        };
        let previous = (
            self.principals[index].pregranted.clone(),
            self.principals[index].gated.clone(),
        );
        self.principals[index].pregranted = pregranted;
        self.principals[index].gated = gated;
        if let Err(error) = self.save() {
            self.principals[index].pregranted = previous.0;
            self.principals[index].gated = previous.1;
            return Err(error);
        }
        Ok(())
    }

    /// Register a principal ahead of time (administrator pre-provisioning):
    /// the same sanitization as pairing, without the interactive prompt.
    /// Returns the issued principal id and credential.
    pub fn register(
        &mut self,
        label: Option<&str>,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    ) -> Result<(String, String), String> {
        let (pregranted, gated) = validate_explicit_ceiling(pregranted, gated)?;
        self.register_record(label, pregranted, gated)
    }

    /// Shared tail of [`PrincipalRegistry::issue`] and
    /// [`PrincipalRegistry::register`]: push a record with a fresh id and
    /// credential and persist. The credential is returned once; the
    /// registry keeps only its digest.
    fn register_record(
        &mut self,
        label: Option<&str>,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    ) -> Result<(String, String), String> {
        validate_label(label)?;
        let principal = format!("prin_{}", random_hex(8)?);
        let mut credential = random_hex(32)?;
        self.principals.push(PrincipalRecord {
            id: principal.clone(),
            label: label.map(str::to_owned),
            credential_sha256: sha256_hex(credential.as_bytes()),
            pregranted,
            gated,
            created_at: now_epoch(),
        });
        if let Err(error) = self.save() {
            self.principals.pop();
            use zeroize::Zeroize as _;
            credential.zeroize();
            return Err(error);
        }
        Ok((principal, credential))
    }

    /// Persist the registry atomically with owner-only permissions. A
    /// session-only registry has nothing to persist.
    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = RegistryFile {
            version: REGISTRY_VERSION,
            principals: self.principals.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| format!("serialize agent registry: {error}"))?;
        atomic_write(path, &bytes)
    }
}

const GRANTS_VERSION: u32 = 1;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct GrantsFile {
    version: u32,
    #[serde(default)]
    grants: Vec<GrantRecord>,
}

/// One durable runtime-grant decision (ADR-0088).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrantRecord {
    pub principal: String,
    pub op: ActorCapability,
    pub allow: bool,
    pub granted_at: u64,
}

/// The compositor-held runtime-grant store (ADR-0088). Durable decisions
/// live in `$XDG_DATA_HOME/tessera/grants.json` with the registry's
/// owner-only, atomic-write, fail-closed discipline; session decisions
/// live only in memory and die with the compositor.
pub struct GrantStore {
    path: Option<PathBuf>,
    /// Durable decisions, persisted on every change.
    grants: Vec<GrantRecord>,
    /// Session-only decisions `(principal, op, allow)`. `ActorCapability` has no
    /// `Hash`, so lookup is linear over this short vec.
    session: Vec<(String, ActorCapability, bool)>,
}

impl GrantStore {
    /// Load the store from `path`. A missing file starts empty; a corrupt
    /// or version-mismatched file also starts empty (fail-closed) and is
    /// logged — never silently trusted.
    pub fn load(path: PathBuf) -> Self {
        let grants = match read_private_state(&path) {
            Ok(Some(bytes)) => match serde_json::from_slice::<GrantsFile>(&bytes) {
                Ok(file) if file.version == GRANTS_VERSION && valid_grants_file(&file) => {
                    file.grants
                }
                Ok(file) => {
                    log::warn!(
                        "agent grants {}: unsupported or invalid version {}, starting empty",
                        path.display(),
                        file.version
                    );
                    Vec::new()
                }
                Err(error) => {
                    log::warn!(
                        "agent grants {}: unreadable ({error}), starting empty",
                        path.display()
                    );
                    Vec::new()
                }
            },
            Ok(None) => Vec::new(),
            Err(error) => {
                log::warn!(
                    "agent grants {}: unsafe or unreadable ({error}), starting empty",
                    path.display()
                );
                Vec::new()
            }
        };
        Self {
            path: Some(path),
            grants,
            session: Vec::new(),
        }
    }

    /// A session-only store for sessions without a durable data directory.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            grants: Vec::new(),
            session: Vec::new(),
        }
    }

    /// The recorded decision for one principal and operation, if any.
    /// Session decisions win over durable ones.
    pub fn decision_for(&self, principal: &str, op: ActorCapability) -> Option<bool> {
        if let Some((_, _, allow)) = self
            .session
            .iter()
            .find(|(p, o, _)| p == principal && *o == op)
        {
            return Some(*allow);
        }
        self.grants
            .iter()
            .find(|record| record.principal == principal && record.op == op)
            .map(|record| record.allow)
    }

    /// Record one decision. `session: true` keeps it in memory only;
    /// otherwise the durable record is updated or inserted (superseding any
    /// session decision for the same pair) and persisted.
    pub fn record(
        &mut self,
        principal: &str,
        op: ActorCapability,
        allow: bool,
        session: bool,
    ) -> Result<(), String> {
        if session {
            if let Some(entry) = self
                .session
                .iter_mut()
                .find(|(p, o, _)| p == principal && *o == op)
            {
                entry.2 = allow;
            } else {
                self.session.push((principal.to_owned(), op, allow));
            }
            return Ok(());
        }
        self.session
            .retain(|(p, o, _)| !(p == principal && *o == op));
        let previous_grants = self.grants.clone();
        let granted_at = now_epoch();
        if let Some(record) = self
            .grants
            .iter_mut()
            .find(|record| record.principal == principal && record.op == op)
        {
            record.allow = allow;
            record.granted_at = granted_at;
        } else {
            self.grants.push(GrantRecord {
                principal: principal.to_owned(),
                op,
                allow,
                granted_at,
            });
        }
        if let Err(error) = self.save() {
            self.grants = previous_grants;
            return Err(error);
        }
        Ok(())
    }

    /// Drop one decision from both scopes and persist. Revoking a grant
    /// that was never recorded is not an error.
    pub fn revoke(&mut self, principal: &str, op: ActorCapability) -> Result<(), String> {
        let previous_session = self.session.clone();
        let previous_grants = self.grants.clone();
        self.session
            .retain(|(p, o, _)| !(p == principal && *o == op));
        self.grants
            .retain(|record| !(record.principal == principal && record.op == op));
        if let Err(error) = self.save() {
            self.session = previous_session;
            self.grants = previous_grants;
            return Err(error);
        }
        Ok(())
    }

    /// Drop every decision belonging to one principal from both scopes and
    /// persist. A persistence failure is logged, not propagated: the
    /// principal is already gone from the registry either way.
    pub fn forget_principal(&mut self, principal: &str) {
        self.session.retain(|(p, _, _)| p != principal);
        self.grants.retain(|record| record.principal != principal);
        if let Err(error) = self.save() {
            log::warn!("agent grants: forget {principal}: {error}");
        }
    }

    /// List durable decisions, optionally filtered to one principal.
    pub fn list(&self, filter: Option<&str>) -> Vec<tessera_protocol::AgentGrantInfo> {
        self.grants
            .iter()
            .filter(|record| filter.is_none_or(|principal| record.principal == principal))
            .map(|record| tessera_protocol::AgentGrantInfo {
                principal: record.principal.clone(),
                op: record.op,
                decision: if record.allow {
                    tessera_protocol::AgentGrantDecision::Allow
                } else {
                    tessera_protocol::AgentGrantDecision::Deny
                },
                granted_at: record.granted_at,
            })
            .collect()
    }

    /// Persist the store atomically with owner-only permissions. A
    /// session-only store has nothing to persist.
    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = GrantsFile {
            version: GRANTS_VERSION,
            grants: self.grants.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| format!("serialize agent grants: {error}"))?;
        atomic_write(path, &bytes)
    }
}

/// One requestable operation projected for the pairing checklist: a stable
/// machine key (the `ActorCapability` variant name, parseable back with
/// [`ActorCapability::from_name`]), a human label, and whether first use is
/// runtime-gated (ADR-0088).
pub struct CapabilityGroupData {
    pub key: &'static str,
    pub label: &'static str,
    pub gated: bool,
}

/// Display families over the agent-requestable operations: the pairing
/// checklist shows one row per family with the operations as expandable
/// detail, so a broad request reads as a handful of intent areas instead of
/// a wall of checkboxes. Every agent-requestable operation belongs to
/// exactly one family (enforced by a test).
const CAPABILITY_FAMILIES: &[(&str, &str, &[ActorCapability])] = &[
    (
        "observe",
        "Observe the desktop",
        &[
            ActorCapability::ObserveWindows,
            ActorCapability::ObserveWorkspaces,
            ActorCapability::ObserveOutputs,
            ActorCapability::ObserveNotifications,
            ActorCapability::ObserveJournal,
            ActorCapability::ObserveSettings,
            ActorCapability::ObserveSystem,
        ],
    ),
    (
        "windows",
        "Control windows",
        &[
            ActorCapability::Focus,
            ActorCapability::Minimize,
            ActorCapability::Close,
            ActorCapability::Move,
            ActorCapability::SetWindowGeometry,
            ActorCapability::Cycle,
            ActorCapability::ToggleOverview,
        ],
    ),
    (
        "workspaces",
        "Manage workspaces",
        &[
            ActorCapability::SwitchWorkspace,
            ActorCapability::SwitchWorkspaceTo,
            ActorCapability::MoveToWorkspace,
        ],
    ),
    (
        "domains",
        "Act in Interaction Domains",
        &[
            ActorCapability::ObserveInteractionDomains,
            ActorCapability::ObserveInteractionDomain,
            ActorCapability::CreateInteractionDomain,
            ActorCapability::TransactInteractionDomain,
            ActorCapability::RevokeInteractionDomain,
            ActorCapability::InjectInteractionDomainInput,
            ActorCapability::CaptureInteractionDomain,
            ActorCapability::LaunchInInteractionDomain,
        ],
    ),
    (
        "notify",
        "Send notifications",
        &[
            ActorCapability::Notify,
            ActorCapability::DismissNotification,
        ],
    ),
    (
        "launch",
        "Launch applications",
        &[ActorCapability::LaunchApp],
    ),
    (
        "capture",
        "Capture window contents",
        &[ActorCapability::CaptureWindow],
    ),
    (
        "data",
        "Files, network, and secrets",
        &[
            ActorCapability::ReadFile,
            ActorCapability::WriteFile,
            ActorCapability::AccessNetworkOrigin,
            ActorCapability::PromptSecret,
            ActorCapability::RequestPayment,
        ],
    ),
];

/// One display family projected for the pairing checklist: a stable machine
/// key, a human label, and the requested operations in display order.
pub struct CapabilityFamilyData {
    pub key: &'static str,
    pub label: &'static str,
    pub members: Vec<CapabilityGroupData>,
}

/// The checklist rows for a pairing request: the requested set filtered
/// down to the agent-requestable operations, grouped into display families,
/// each operation marked with its runtime-gated flag. Families with no
/// requested operation drop out.
pub fn capability_families(requested: &[ActorCapability]) -> Vec<CapabilityFamilyData> {
    CAPABILITY_FAMILIES
        .iter()
        .filter_map(|(key, label, ops)| {
            let members: Vec<CapabilityGroupData> = ops
                .iter()
                .filter(|op| requested.contains(op))
                .map(|op| CapabilityGroupData {
                    key: op_key(*op),
                    label: op.label(),
                    gated: is_runtime_gated(*op),
                })
                .collect();
            (!members.is_empty()).then_some(CapabilityFamilyData {
                key,
                label,
                members,
            })
        })
        .collect()
}

/// The stable machine key of one agent-requestable operation: its
/// `ActorCapability` variant name, which [`ActorCapability::from_name`]
/// parses back.
fn op_key(op: ActorCapability) -> &'static str {
    match op {
        ActorCapability::ObserveWindows => "ObserveWindows",
        ActorCapability::ObserveWorkspaces => "ObserveWorkspaces",
        ActorCapability::ObserveOutputs => "ObserveOutputs",
        ActorCapability::ObserveNotifications => "ObserveNotifications",
        ActorCapability::ObserveJournal => "ObserveJournal",
        ActorCapability::ObserveInteractionDomains => "ObserveInteractionDomains",
        ActorCapability::ObserveSettings => "ObserveSettings",
        ActorCapability::ObserveSystem => "ObserveSystem",
        ActorCapability::Focus => "Focus",
        ActorCapability::Minimize => "Minimize",
        ActorCapability::Close => "Close",
        ActorCapability::Move => "Move",
        ActorCapability::SetWindowGeometry => "SetWindowGeometry",
        ActorCapability::Cycle => "Cycle",
        ActorCapability::SwitchWorkspace => "SwitchWorkspace",
        ActorCapability::SwitchWorkspaceTo => "SwitchWorkspaceTo",
        ActorCapability::MoveToWorkspace => "MoveToWorkspace",
        ActorCapability::ToggleOverview => "ToggleOverview",
        ActorCapability::Notify => "Notify",
        ActorCapability::DismissNotification => "DismissNotification",
        ActorCapability::ReadFile => "ReadFile",
        ActorCapability::WriteFile => "WriteFile",
        ActorCapability::AccessNetworkOrigin => "AccessNetworkOrigin",
        ActorCapability::PromptSecret => "PromptSecret",
        ActorCapability::RequestPayment => "RequestPayment",
        ActorCapability::InjectInteractionDomainInput => "InjectInteractionDomainInput",
        ActorCapability::CreateInteractionDomain => "CreateInteractionDomain",
        ActorCapability::TransactInteractionDomain => "TransactInteractionDomain",
        ActorCapability::RevokeInteractionDomain => "RevokeInteractionDomain",
        ActorCapability::CaptureInteractionDomain => "CaptureInteractionDomain",
        ActorCapability::CaptureWindow => "CaptureWindow",
        ActorCapability::ObserveInteractionDomain => "ObserveInteractionDomain",
        ActorCapability::LaunchInInteractionDomain => "LaunchInInteractionDomain",
        ActorCapability::LaunchApp => "LaunchApp",
        // Component-only families never reach the pairing checklist:
        // `capability_families` filters them out first.
        _ => unreachable!("op_key is only called for agent-requestable ops"),
    }
}

fn deny_key(label: Option<&str>) -> String {
    label
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "(unlabeled)".into())
}

/// Validate an administrator-supplied ceiling without silently weakening its
/// meaning. Runtime-gated families may never be placed in `pregranted`, and
/// duplicate, overlapping, or component-only families are rejected.
fn validate_explicit_ceiling(
    pregranted: Vec<ActorCapability>,
    gated: Vec<ActorCapability>,
) -> Result<(Vec<ActorCapability>, Vec<ActorCapability>), String> {
    let validate_group = |name: &str, ops: &[ActorCapability]| -> Result<(), String> {
        let mut seen = Vec::new();
        for op in ops {
            if !AGENT_REQUESTABLE.contains(op) {
                return Err(format!("{name} contains component-only operation {op:?}"));
            }
            if seen.contains(op) {
                return Err(format!("{name} contains duplicate operation {op:?}"));
            }
            seen.push(*op);
        }
        Ok(())
    };
    validate_group("pregranted", &pregranted)?;
    validate_group("gated", &gated)?;
    if let Some(op) = pregranted.iter().find(|op| is_runtime_gated(**op)) {
        return Err(format!(
            "operation {op:?} is security-sensitive and must be runtime-gated"
        ));
    }
    if let Some(op) = pregranted.iter().find(|op| gated.contains(op)) {
        return Err(format!(
            "operation {op:?} cannot be both pregranted and runtime-gated"
        ));
    }
    Ok((pregranted, gated))
}

fn validate_label(label: Option<&str>) -> Result<(), String> {
    if label.is_some_and(|label| {
        label.trim().is_empty() || label.len() > 256 || label.chars().any(char::is_control)
    }) {
        return Err("Agent label is empty, oversized, or contains control characters".into());
    }
    Ok(())
}

fn valid_registry_file(file: &RegistryFile) -> bool {
    if file.principals.len() > MAX_PRINCIPALS {
        return false;
    }
    file.principals.iter().enumerate().all(|(index, record)| {
        tessera_authority::authority::ActorPrincipal::new(record.id.clone()).is_ok()
            && validate_label(record.label.as_deref()).is_ok()
            && record.credential_sha256.len() == 64
            && record
                .credential_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && validate_explicit_ceiling(record.pregranted.clone(), record.gated.clone()).is_ok()
            && !file.principals[..index].iter().any(|previous| {
                previous.id == record.id || previous.credential_sha256 == record.credential_sha256
            })
    })
}

fn valid_grants_file(file: &GrantsFile) -> bool {
    file.grants.len() <= MAX_GRANTS
        && file.grants.iter().enumerate().all(|(index, record)| {
            tessera_authority::authority::ActorPrincipal::new(record.principal.clone()).is_ok()
                && AGENT_REQUESTABLE.contains(&record.op)
                && !file.grants[..index].iter().any(|previous| {
                    previous.principal == record.principal && previous.op == record.op
                })
        })
}

fn read_private_state(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("open {}: {error}", path.display())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > MAX_STATE_BYTES
    {
        return Err(format!(
            "{} has unsafe ownership, mode, type, link count, or size",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(Some(bytes))
}

/// Crash-durable, owner-only replacement used by both authorization stores.
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect {}: {error}", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale {}: {error}", tmp.display())),
    }
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|error| format!("create {}: {error}", tmp.display()))?;
    handle
        .write_all(bytes)
        .map_err(|error| format!("write {}: {error}", tmp.display()))?;
    handle
        .sync_all()
        .map_err(|error| format!("sync {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("replace {}: {error}", path.display()))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync {}: {error}", parent.display()))
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn random_hex(bytes: usize) -> Result<String, String> {
    use zeroize::Zeroize as _;
    let mut buf = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut buf))
        .map_err(|error| format!("read /dev/urandom: {error}"))?;
    let encoded = buf.iter().map(|byte| format!("{byte:02x}")).collect();
    buf.zeroize();
    Ok(encoded)
}

#[cfg(test)]
mod tests;

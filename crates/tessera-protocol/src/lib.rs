//! Versioned Tessera wire schema and audit vocabulary. No sockets or persistence.
#![forbid(unsafe_code)]
pub mod journal;
pub mod schema;
pub use tessera_authority::authority::{
    ActorPrincipal, ActorResource, ActorSessionId, ActorSessionPolicy, ActorSessionSnapshot,
    ActorSessionState, FilesystemAccess, ResourceGrant, ResourceGrantId,
};

pub use journal::{
    ActorSessionAuditAction, AgentAuthAction, AuditedCommand, AuditedSemanticAction,
    CapabilityUseAction, Effect, GrantPersistence, JournalEntry, JournalMutation, JournalSnapshot,
    Origin, ResourceGrantAttemptAction, ResourceGrantAuditAction, ResourceKind,
    audit_semantic_actions,
};
pub use schema::{
    AccessibilityTreeUpdate, AccessibilityWindowBinding, ActorActionIntent, ActorActionReceipt,
    ActorCapability, AgentGrantDecision, AgentGrantInfo, AgentHello, AgentIssued,
    AgentPrincipalInfo, AppPickResult, AuthorizationDecision, Command, CommandScopePolicy,
    ConfirmPickResult, ConnectionCapabilities, Event, InteractionDomainAction,
    InteractionDomainActionResult, InteractionDomainCapture, JournalCursor,
    LOCAL_AGENT_ADMIN_SCOPE, LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE, LOCAL_OWNER_ADMIN_SCOPE,
    LOCAL_PORTAL_SCOPE, LeaseGrant, LeaseRequest, MAX_TRANSACT_OPS, ObservationToken,
    ObserveSnapshot, OutputInfo, PROTOCOL_VERSION, PickKind, PickResult, Request, Response, Scope,
    SecretPromptResult, SemanticActionRequest, SemanticObservation, SettingsAction,
    SettingsReceipt, SettingsSnapshot, StreamCursorMode, StreamPixelFormat, StreamTarget,
    SystemAction, SystemStatus, TransactOp, TransactOpResult, TransactPrecondition,
    TransactReceipt, TransactResult, WindowCapture,
};

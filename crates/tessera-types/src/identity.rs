/// Stable identifier for a window, opaque to chrome/IPC/agent. Allocated
/// monotonically by the compositor and never reused within the process
/// lifetime (ADR-0032). Outlives the surface: a retired id remains valid as
/// a journal or scope reference but is never reassigned.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WindowId(pub u64);

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub u64);

/// Stable identifier for an output.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(pub u64);

macro_rules! durable_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl $name {
            /// Identifier zero is reserved as the invalid/unassigned value.
            pub fn is_valid(self) -> bool {
                self.0 != 0
            }
        }
    };
}

durable_id!(
    InteractionPrincipalId,
    "Durable compositor-model identity of a human, agent, or system principal. Distinct from an authenticated ActorPrincipal."
);
durable_id!(
    InteractionDomainId,
    "Durable identity of one authority and presentation domain."
);
durable_id!(
    SeatId,
    "Durable identity of one independent logical input seat."
);
durable_id!(
    ClientId,
    "Durable identity of one Wayland client connection."
);
durable_id!(
    InteractionGroupId,
    "Durable identity of one atomically transferable interactive surface group."
);

/// The bootstrap human principal exists for the compositor lifetime.
pub const HUMAN_PRINCIPAL: InteractionPrincipalId = InteractionPrincipalId(1);
/// The physical desktop's authority domain.
pub const HUMAN_INTERACTION_DOMAIN: InteractionDomainId = InteractionDomainId(1);
/// The physical user's logical seat.
pub const HUMAN_SEAT: SeatId = SeatId(1);

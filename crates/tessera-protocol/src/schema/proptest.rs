//! Property tests for the wire schema.
//!
//! The IPC wire is a versioned, self-describing serde surface: every message is
//! JSON with an internal `{"type": …}` tag, a peer that predates a field must
//! still decode it (serde defaults), and an unknown tag must be rejected rather
//! than silently defaulted. Example-based round-trips live in `tests.rs`; these
//! properties assert the *invariant* over generated values, which is where a
//! misplaced serde attribute actually surfaces.
//!
//! Property testing is applied here — and to the other wire/codec parsers —
//! because generated cases pay off where the surface is a versioned encoding
//! or a strict parser, and add nothing for layout or motion code.

use ::proptest::prelude::*;

use super::*;

/// Serialize `value` and parse it straight back — the wire round-trip.
fn round_trip<T>(value: &T) -> Result<T, serde_json::Error>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(value)?)
}

/// The JSON tag of a `#[serde(tag = "type")]` value.
fn wire_tag<T: serde::Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");
    parsed
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn pick_kind() -> impl Strategy<Value = PickKind> {
    prop_oneof![
        Just(PickKind::Region),
        Just(PickKind::Pixel),
        Just(PickKind::Window),
        Just(PickKind::Output),
    ]
}

fn transact_precondition() -> impl Strategy<Value = TransactPrecondition> {
    prop_oneof![
        Just(TransactPrecondition::JournalSeq),
        Just(TransactPrecondition::InteractionDomainRevision),
    ]
}

fn agent_grant_decision() -> impl Strategy<Value = AgentGrantDecision> {
    prop_oneof![
        Just(AgentGrantDecision::Allow),
        Just(AgentGrantDecision::Deny)
    ]
}

proptest! {
    /// Capability negotiation is an object of named booleans on the wire, so a
    /// remote tool can read it without decoding a bitmask, and it survives the
    /// round-trip for every combination.
    #[test]
    fn connection_capabilities_round_trip(bits in any::<[bool; 5]>()) {
        let value = ConnectionCapabilities {
            query: bits[0],
            control: bits[1],
            input: bits[2],
            session: bits[3],
            interaction_domain: bits[4],
        };
        prop_assert_eq!(round_trip(&value).expect("round-trip"), value);
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&value).expect("serialize")).expect("parse");
        prop_assert!(json.is_object());
        for field in ["query", "control", "input", "session", "interaction_domain"] {
            prop_assert!(json.get(field).is_some_and(serde_json::Value::is_boolean));
        }
    }

    /// Every scalar DTO and tagged enum survives `to_string` → `from_str`
    /// unchanged, for the full `u64` range (so no field silently truncates).
    #[test]
    fn scalar_dtos_and_enums_round_trip(
        ttl in any::<u64>(),
        id in any::<u64>(),
        renewable in any::<bool>(),
        oldest in any::<u64>(),
        latest in any::<u64>(),
        kind in pick_kind(),
        precondition in transact_precondition(),
        decision in agent_grant_decision(),
    ) {
        prop_assert_eq!(round_trip(&LeaseRequest { ttl_ms: ttl }).expect("lease request"), LeaseRequest { ttl_ms: ttl });
        prop_assert_eq!(
            round_trip(&LeaseGrant { id, ttl_ms: ttl, renewable }).expect("lease grant"),
            LeaseGrant { id, ttl_ms: ttl, renewable }
        );
        prop_assert_eq!(
            round_trip(&JournalCursor { oldest_seq: oldest, latest_seq: latest }).expect("cursor"),
            JournalCursor { oldest_seq: oldest, latest_seq: latest }
        );
        prop_assert_eq!(round_trip(&kind).expect("pick kind"), kind);
        prop_assert_eq!(round_trip(&precondition).expect("precondition"), precondition);
        prop_assert_eq!(round_trip(&decision).expect("decision"), decision);
        prop_assert!(!wire_tag(&kind).is_empty());
        prop_assert!(!wire_tag(&precondition).is_empty());
        prop_assert!(!wire_tag(&decision).is_empty());
    }
}

/// A newer peer's unknown enum tag must be refused, not coerced to a default:
/// the wire is strict about variants even where individual fields default.
#[test]
fn unknown_enum_tags_are_rejected() {
    assert!(serde_json::from_str::<PickKind>(r#"{"type":"NotAKind"}"#).is_err());
    assert!(serde_json::from_str::<TransactPrecondition>(r#"{"type":"Nope"}"#).is_err());
    assert!(serde_json::from_str::<AgentGrantDecision>(r#"{"type":"Maybe"}"#).is_err());
}

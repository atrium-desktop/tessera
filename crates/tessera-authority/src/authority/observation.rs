use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::interaction_domain::InteractionDomainId;
use tessera_primitives::input::SyntheticInputAction;
use tessera_primitives::{Rect, Size, WindowId};

use super::{ActorPrincipal, ActorSessionId};

const OBSERVATION_TTL: Duration = Duration::from_secs(15);
const MAX_LIVE_OBSERVATIONS: usize = 1024;
const MAX_RETAINED_WINDOWS: usize = 4_096;
const DEFAULT_MAX_OBSERVATIONS_PER_ACTOR: usize = 64;

/// One authenticated Actor bound to a live broker connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorBinding {
    pub session: ActorSessionId,
    pub connection_id: u64,
    pub principal: Option<ActorPrincipal>,
}

/// Opaque bearer reference to one compositor-owned observation lease.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ObservationToken(pub String);

/// An observed toplevel window in an Interaction Domain.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ObservedWindow {
    pub id: WindowId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Window placement on the Interaction Domain's virtual output.
    pub bounds: Rect,
    /// Surface local extent for bounded coordinate validation.
    pub local_size: Size,
    pub visible: bool,
    pub focused: bool,
    pub enabled: bool,
    pub minimized: bool,
    pub read_only: bool,
    pub revision: u64,
}

/// Observation snapshot captured atomically with an Interaction Domain observation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ObservationSnapshot {
    pub interaction_domain: InteractionDomainId,
    pub authority_revision: u64,
    pub windows: Vec<ObservedWindow>,
}

impl ObservationSnapshot {
    pub fn window(&self, id: WindowId) -> Option<&ObservedWindow> {
        self.windows.iter().find(|window| window.id == id)
    }
}

/// An interaction-domain observation usable as one action transaction's
/// precondition without granting unbounded hardware access.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InteractionDomainObservation {
    pub token: ObservationToken,
    /// Remaining lease time when issued. Informational only; the compositor's
    /// monotonic deadline remains authoritative.
    pub ttl_ms: u64,
    pub snapshot: ObservationSnapshot,
}

/// Observation-bound action intent submitted by an Actor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActorActionIntent {
    pub interaction_domain: InteractionDomainId,
    pub target_window: WindowId,
    pub observation: ObservationToken,
    pub actions: Vec<SyntheticInputAction>,
}

impl ActorActionIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        const MAX_INPUT_ACTIONS: usize = 64;
        const MAX_SCROLL_DELTA: f32 = 1_000.0;
        if !self.interaction_domain.is_valid() || self.target_window.0 == 0 {
            return Err("interaction domain or target window is invalid");
        }
        if self.observation.0.len() < 32 || self.observation.0.len() > 128 {
            return Err("observation token length is out of range");
        }
        if self.actions.is_empty() || self.actions.len() > MAX_INPUT_ACTIONS {
            return Err("input action count is out of range");
        }
        for action in &self.actions {
            match *action {
                SyntheticInputAction::Click { button, .. }
                    if !(0x110..=0x117).contains(&button) =>
                {
                    return Err("input button code is out of range");
                }
                SyntheticInputAction::Scroll { dx, dy, .. }
                    if !dx.is_finite()
                        || !dy.is_finite()
                        || dx.abs() > MAX_SCROLL_DELTA
                        || dy.abs() > MAX_SCROLL_DELTA =>
                {
                    return Err("input scroll delta is out of range");
                }
                SyntheticInputAction::KeyPress { code } if code > 0x2ff => {
                    return Err("input key code is out of range");
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Authoritative receipt created only after the compositor main loop has
/// revalidated and delivered the action batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActorActionReceipt {
    pub action_id: u64,
    pub interaction_domain: InteractionDomainId,
    pub target_window: WindowId,
    pub authority_revision: u64,
    pub actions_applied: u32,
    pub committed_mono_ms: u64,
}

struct ObservationLease {
    actor: ActorBinding,
    snapshot: ObservationSnapshot,
    expires_at: Instant,
    max_observations: usize,
}

/// Bounded, main-loop-owned observation leases.
#[derive(Default)]
pub struct ObservationLeaseRegistry {
    leases: BTreeMap<ObservationToken, ObservationLease>,
    next_action_id: u64,
}

#[derive(Debug)]
pub struct ValidatedActorAction {
    pub action_id: u64,
    pub window: WindowId,
    pub authority_revision: u64,
}

impl ObservationLeaseRegistry {
    pub fn discard_all(&mut self) {
        self.leases.clear();
    }

    pub fn discard(&mut self, token: &ObservationToken) {
        self.leases.remove(token);
    }

    pub fn discard_connection(&mut self, connection_id: u64) {
        self.leases
            .retain(|_, lease| lease.actor.connection_id != connection_id);
    }

    pub fn discard_for_actor(&mut self, actor: &ActorBinding, token: &ObservationToken) {
        if self
            .leases
            .get(token)
            .is_some_and(|lease| lease.actor == *actor)
        {
            self.leases.remove(token);
        }
    }

    pub fn issue(
        &mut self,
        actor: ActorBinding,
        snapshot: ObservationSnapshot,
    ) -> Result<InteractionDomainObservation, String> {
        self.issue_bounded(actor, snapshot, DEFAULT_MAX_OBSERVATIONS_PER_ACTOR)
    }

    pub fn issue_bounded(
        &mut self,
        actor: ActorBinding,
        snapshot: ObservationSnapshot,
        max_observations: usize,
    ) -> Result<InteractionDomainObservation, String> {
        if max_observations == 0 || max_observations > 4_096 {
            return Err("Actor observation quota is out of range".into());
        }
        let now = Instant::now();
        self.leases.retain(|_, lease| lease.expires_at > now);
        if self
            .leases
            .values()
            .filter(|lease| lease.actor.session == actor.session)
            .count()
            >= max_observations
        {
            return Err("Actor observation quota exhausted".into());
        }
        if snapshot.windows.len() > MAX_RETAINED_WINDOWS {
            return Err("observation exceeds the retained-window safety bound".into());
        }
        while self.leases.len() >= MAX_LIVE_OBSERVATIONS
            || self
                .leases
                .values()
                .map(|lease| lease.snapshot.windows.len())
                .sum::<usize>()
                .saturating_add(snapshot.windows.len())
                > MAX_RETAINED_WINDOWS
        {
            let Some(oldest) = self
                .leases
                .iter()
                .min_by_key(|(_, lease)| lease.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.leases.remove(&oldest);
        }

        let token = loop {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|error| format!("generate observation token: {error}"))?;
            let mut encoded = String::with_capacity(bytes.len() * 2);
            use std::fmt::Write as _;
            for byte in bytes {
                write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
            }
            let token = ObservationToken(encoded);
            if !self.leases.contains_key(&token) {
                break token;
            }
        };
        self.leases.insert(
            token.clone(),
            ObservationLease {
                actor,
                snapshot: snapshot.clone(),
                expires_at: now + OBSERVATION_TTL,
                max_observations,
            },
        );
        Ok(InteractionDomainObservation {
            token,
            ttl_ms: OBSERVATION_TTL.as_millis() as u64,
            snapshot,
        })
    }

    pub fn refresh_for_delivery(
        &mut self,
        token: &ObservationToken,
    ) -> Result<InteractionDomainObservation, String> {
        let lease = self
            .leases
            .remove(token)
            .ok_or_else(|| "capture observation was revoked before delivery".to_owned())?;
        self.issue_bounded(lease.actor, lease.snapshot, lease.max_observations)
    }

    pub fn consume(
        &mut self,
        actor: &ActorBinding,
        intent: &ActorActionIntent,
        current: &ObservationSnapshot,
        permits_window: impl FnOnce(WindowId) -> bool,
    ) -> Result<ValidatedActorAction, String> {
        let bound_actor = &self
            .leases
            .get(&intent.observation)
            .ok_or_else(|| "unknown, expired, or already consumed observation".to_owned())?
            .actor;
        if bound_actor != actor {
            return Err("observation belongs to a different Actor connection".into());
        }
        let lease = self
            .leases
            .remove(&intent.observation)
            .expect("the Actor-bound observation was present immediately before removal");
        if lease.expires_at <= Instant::now() {
            return Err("observation expired before action commit".into());
        }
        if lease.snapshot.interaction_domain != intent.interaction_domain
            || current.interaction_domain != intent.interaction_domain
        {
            return Err("observation domain does not match action domain".into());
        }
        if lease.snapshot.authority_revision != current.authority_revision {
            return Err(format!(
                "interaction-domain authority changed after observation (observed r{}, current r{})",
                lease.snapshot.authority_revision, current.authority_revision
            ));
        }
        let observed = lease
            .snapshot
            .window(intent.target_window)
            .ok_or_else(|| "target window was not present in the observation".to_owned())?;
        let current_window = current
            .window(intent.target_window)
            .ok_or_else(|| "target window no longer exists".to_owned())?;
        if !permits_window(current_window.id) {
            return Err("target window is out of scope".into());
        }
        if observed != current_window {
            return Err("target window state changed after observation".into());
        }
        if !current_window.visible || !current_window.enabled || current_window.read_only {
            return Err("target window is not actionable by this interaction domain".into());
        }
        for action in &intent.actions {
            let position = action.pointer_position();
            if position.is_some_and(|position| {
                position.x < 0
                    || position.y < 0
                    || position.x >= current_window.local_size.w
                    || position.y >= current_window.local_size.h
            }) {
                return Err("action position is outside the window extent".into());
            }
        }
        self.next_action_id = self
            .next_action_id
            .checked_add(1)
            .ok_or_else(|| "Actor action id space exhausted".to_owned())?;
        Ok(ValidatedActorAction {
            action_id: self.next_action_id,
            window: current_window.id,
            authority_revision: current.authority_revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(connection_id: u64, principal: &str) -> ActorBinding {
        ActorBinding {
            session: ActorSessionId(connection_id),
            connection_id,
            principal: Some(ActorPrincipal::new(principal).unwrap()),
        }
    }

    fn snapshot(domain: u64, authority_revision: u64, object_revision: u64) -> ObservationSnapshot {
        ObservationSnapshot {
            interaction_domain: InteractionDomainId(domain),
            authority_revision,
            windows: vec![ObservedWindow {
                id: WindowId(9),
                app_id: Some("shop.example".into()),
                title: Some("Checkout".into()),
                bounds: Rect::new(0, 0, 800, 600),
                local_size: Size { w: 800, h: 600 },
                visible: true,
                focused: true,
                enabled: true,
                minimized: false,
                read_only: false,
                revision: object_revision,
            }],
        }
    }

    fn intent(observation: ObservationToken) -> ActorActionIntent {
        ActorActionIntent {
            interaction_domain: InteractionDomainId(7),
            target_window: WindowId(9),
            observation,
            actions: vec![SyntheticInputAction::PointerMove {
                position: tessera_primitives::Point { x: 20, y: 30 },
            }],
        }
    }

    #[test]
    fn observation_is_actor_bound_and_single_use() {
        let mut registry = ObservationLeaseRegistry::default();
        let owner = actor(4, "prin_a");
        let observed = snapshot(7, 11, 3);
        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        let action = intent(lease.token);
        let validated = registry
            .consume(&owner, &action, &observed, |_| true)
            .unwrap();
        assert_eq!(validated.window, WindowId(9));
        assert!(
            registry
                .consume(&owner, &action, &observed, |_| true)
                .is_err()
        );
    }
}

//! Consumer-ownership state of one capture-surface slot ring.

use std::time::Duration;

/// Capture-surface slot ring depth: the device-wide flux frames-in-flight
/// count the compositor's DRM device is created with.
pub const STREAM_SLOT_COUNT: usize = 3;

/// A slot acquire fence that never signals means the GPU wedged; drop the
/// frame and recycle the slot rather than stalling the ring forever.
pub const SLOT_FENCE_TIMEOUT: Duration = Duration::from_secs(1);

/// Consumer-ownership state of one capture-surface slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Available for the next rendered frame.
    Free,
    /// Rendered and exported; its acquire fence has not signaled yet, so no
    /// frame event references it.
    Rendering,
    /// Frame event delivered; the consumer owns the slot until
    /// `StreamBufferRelease`.
    Pinned,
}

/// Ring position and per-slot ownership of one dmabuf stream's capture
/// surface. Flux advances its frame ring in lockstep with submissions
/// starting at slot 0, so the slot a submission lands in is known before
/// `begin_frame`; submitting only into `Free` slots keeps the two rings
/// synchronized and never overwrites a consumer-owned image.
#[derive(Debug)]
pub struct SlotRing {
    pub states: Vec<SlotState>,
    pub next: usize,
}

impl SlotRing {
    pub fn new(count: usize) -> Self {
        Self {
            states: vec![SlotState::Free; count],
            next: 0,
        }
    }

    /// The slot the next submitted frame lands in, or `None` while the
    /// consumer still owns it (the due frame then counts as dropped).
    pub fn next_submission_slot(&self) -> Option<usize> {
        (self.states[self.next] == SlotState::Free).then_some(self.next)
    }

    /// Record a submission into `slot` (the ring position at render time)
    /// and advance the ring.
    pub fn submitted(&mut self, slot: usize) {
        debug_assert_eq!(slot, self.next, "flux ring and slot tracking diverged");
        if slot < self.states.len() {
            self.states[slot] = SlotState::Rendering;
            self.next = (slot + 1) % self.states.len();
        }
    }

    /// A slot's acquire fence signaled: `delivered` distinguishes a frame
    /// handed to the connection lane (consumer-owned from here) from a
    /// backpressure drop (immediately reusable).
    pub fn fence_signaled(&mut self, slot: usize, delivered: bool) {
        if slot < self.states.len() {
            self.states[slot] = if delivered {
                SlotState::Pinned
            } else {
                SlotState::Free
            };
        }
    }

    /// Recycle a slot whose fence timed out: the consumer never saw the
    /// frame, so the slot is reusable without a release.
    pub fn recycle(&mut self, slot: usize) {
        if slot < self.states.len() && self.states[slot] == SlotState::Rendering {
            self.states[slot] = SlotState::Free;
        }
    }

    /// True while the ring's next submission slot is consumer-owned: every
    /// due frame drops until a `StreamBufferRelease` arrives. The two older
    /// slots are necessarily Rendering or Pinned as well, so the ring is
    /// genuinely full.
    pub fn next_is_pinned(&self) -> bool {
        self.states[self.next] == SlotState::Pinned
    }

    /// The consumer finished reading a pinned slot (`StreamBufferRelease`).
    pub fn release(&mut self, slot: u32) {
        if let Some(state) = self.states.get_mut(slot as usize)
            && *state == SlotState::Pinned
        {
            *state = SlotState::Free;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_ring_tracks_consumer_ownership() {
        let mut ring = SlotRing::new(3);
        assert_eq!(ring.next_submission_slot(), Some(0));
        ring.submitted(0);
        assert_eq!(ring.next_submission_slot(), Some(1));
        ring.submitted(1);
        ring.submitted(2);
        // The ring wrapped; slot 0's frame is still rendering.
        assert_eq!(ring.next_submission_slot(), None);

        // A delivered frame pins its slot: a due frame drops rather than
        // overwriting consumer-owned content.
        ring.fence_signaled(0, true);
        assert_eq!(ring.next_submission_slot(), None);
        // A delivery failure frees its slot but the ring position stays.
        ring.fence_signaled(1, false);
        assert_eq!(ring.next_submission_slot(), None);

        // The consumer's release unblocks the ring.
        ring.release(0);
        assert_eq!(ring.next_submission_slot(), Some(0));
        ring.submitted(0);
        assert_eq!(ring.next_submission_slot(), Some(1));
    }

    #[test]
    fn release_only_frees_a_pinned_slot() {
        let mut ring = SlotRing::new(3);
        ring.submitted(0);
        // Still rendering: a release does not apply.
        ring.release(0);
        assert_eq!(ring.states[0], SlotState::Rendering);
        // Out-of-range slots are ignored.
        ring.release(9);
        ring.fence_signaled(0, true);
        ring.release(0);
        assert_eq!(ring.states[0], SlotState::Free);
    }

    #[test]
    fn timed_out_slot_is_recycled_without_a_release() {
        let mut ring = SlotRing::new(2);
        ring.submitted(0);
        ring.submitted(1);
        // Slot 0's fence "timed out" before its delivery: recycled.
        ring.recycle(0);
        assert_eq!(ring.states[0], SlotState::Free);
        assert_eq!(ring.next_submission_slot(), Some(0));
        // A pinned slot is never recycled: the consumer owns it.
        ring.submitted(0);
        ring.fence_signaled(0, true);
        ring.recycle(0);
        assert_eq!(ring.states[0], SlotState::Pinned);
    }
}

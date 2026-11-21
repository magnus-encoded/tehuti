//! The secret confirmation state machine (ADR 0002): one pure step taking
//! button, focus, frame-presented, lookup-result and time, returning effects.
//!
//! The step performs no I/O. The caller runs the lookup and the delivery and
//! reports the lookup result back as an input.

use core::time::Duration;

use tehuti_protocol::{ButtonState, SecretReference};

use crate::secret::{AuthorizedSecretLookup, SecretConfirmationGate, SecretDestination};

/// How long a confirmation request stays answerable.
pub const CONFIRMATION_LIFETIME: Duration = Duration::from_secs(20);
/// How long the "delivery unavailable" notice stays visible.
pub const NOTICE_LIFETIME: Duration = Duration::from_secs(3);

const ENTRY_LABEL_LIMIT: usize = 21;
const TARGET_LABEL_LIMIT: usize = 29;

/// How far a shown confirmation is from accepting the controller button.
///
/// A press only confirms after the user saw the prompt (a frame carrying it
/// was presented) and let go of the button since, so a press that began
/// before the prompt appeared cannot confirm it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arming {
    /// The prompt has not been presented yet.
    AwaitingFrame,
    /// The prompt is presented but the button is still held from before.
    AwaitingRelease,
    /// The next press confirms.
    Armed,
}

/// What the confirmation overlay says; both lines are sanitised text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationPrompt {
    pub entry: String,
    pub target: String,
}

#[derive(Debug)]
enum Phase {
    Idle,
    Confirming {
        arming: Arming,
        prompt: ConfirmationPrompt,
    },
    /// Confirmed; the caller is looking the value up for this destination.
    Looking {
        destination: SecretDestination,
    },
}

/// Everything the step reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretInput {
    /// Ask for a confirmation of `reference` on the current destination.
    Request {
        reference: SecretReference,
        target: String,
    },
    /// Keyboard focus moved: any unanswered confirmation is cancelled.
    FocusChanged,
    /// The confirmation button changed, seen while a confirmation may be shown.
    Button(ButtonState),
    /// The confirmation button changed; tracked even with nothing shown.
    ButtonHeld(bool),
    /// A frame carrying the prompt reached the screen.
    FramePresented,
    /// The caller's lookup ended; `found` is false when the store failed.
    LookupFinished { found: bool },
    /// The caller could not deliver the looked-up value.
    DeliveryFailed,
}

/// What the caller must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretEffect {
    /// Start the lookup, off the compositor thread.
    StartLookup(AuthorizedSecretLookup),
    /// Deliver the found value to this destination, which is still current.
    Deliver(SecretDestination),
    /// The button belonged to the confirmation; do not route it further.
    ConsumeButton,
    /// The request was not accepted.
    Refused,
}

/// Confirmation state for one session.
#[derive(Debug)]
pub struct SecretFlow {
    gate: SecretConfirmationGate,
    phase: Phase,
    button_held: bool,
    notice_until: Option<Duration>,
}

impl SecretFlow {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gate: SecretConfirmationGate::new(),
            phase: Phase::Idle,
            button_held: false,
            notice_until: None,
        }
    }

    /// Advances the machine by one input. `current` is the live destination
    /// (None when nothing is focused) and `now` is the monotonic time.
    pub fn step(
        &mut self,
        input: SecretInput,
        now: Duration,
        current: Option<SecretDestination>,
    ) -> Vec<SecretEffect> {
        match input {
            SecretInput::Request { reference, target } => self.request(reference, &target, now, current),
            SecretInput::FocusChanged => {
                self.gate.cancel();
                if matches!(self.phase, Phase::Confirming { .. }) {
                    self.phase = Phase::Idle;
                }
                Vec::new()
            }
            SecretInput::ButtonHeld(held) => {
                self.button_held = held;
                Vec::new()
            }
            SecretInput::FramePresented => {
                self.frame_presented(now, current);
                Vec::new()
            }
            SecretInput::Button(state) => self.button(state, now, current),
            SecretInput::LookupFinished { found } => self.lookup_finished(found, now, current),
            SecretInput::DeliveryFailed => {
                self.raise_notice(now);
                Vec::new()
            }
        }
    }

    const fn raise_notice(&mut self, now: Duration) {
        self.notice_until = now.checked_add(NOTICE_LIFETIME);
    }

    fn refuse(&mut self, now: Duration) -> Vec<SecretEffect> {
        self.raise_notice(now);
        vec![SecretEffect::Refused]
    }

    fn request(
        &mut self,
        reference: SecretReference,
        target: &str,
        now: Duration,
        current: Option<SecretDestination>,
    ) -> Vec<SecretEffect> {
        let Some(destination) = current else {
            return self.refuse(now);
        };
        if self.looking() {
            return self.refuse(now);
        }
        let prompt = ConfirmationPrompt {
            entry: sanitize_label(&reference.entry, ENTRY_LABEL_LIMIT),
            target: sanitize_label(target, TARGET_LABEL_LIMIT),
        };
        if self
            .gate
            .request(reference, destination, now, CONFIRMATION_LIFETIME)
            .is_none()
        {
            return self.refuse(now);
        }
        self.phase = Phase::Confirming {
            arming: Arming::AwaitingFrame,
            prompt,
        };
        Vec::new()
    }

    fn frame_presented(&mut self, now: Duration, current: Option<SecretDestination>) {
        if self.prompt(now, current).is_none() {
            return;
        }
        let held = self.button_held;
        if let Phase::Confirming { arming, .. } = &mut self.phase {
            *arming = if held {
                Arming::AwaitingRelease
            } else {
                Arming::Armed
            };
        }
    }

    fn button(
        &mut self,
        state: ButtonState,
        now: Duration,
        current: Option<SecretDestination>,
    ) -> Vec<SecretEffect> {
        let Some(destination) = current else {
            return Vec::new();
        };
        if self.prompt(now, current).is_none() {
            return Vec::new();
        }
        let Phase::Confirming { arming, .. } = &mut self.phase else {
            return Vec::new();
        };
        match (state, *arming) {
            (ButtonState::Released, current_arming) => {
                self.button_held = false;
                if current_arming == Arming::AwaitingRelease {
                    *arming = Arming::Armed;
                }
            }
            (ButtonState::Pressed, Arming::Armed) => {
                self.button_held = true;
                return self.confirm(now, destination);
            }
            (ButtonState::Pressed, _) => self.button_held = true,
        }
        vec![SecretEffect::ConsumeButton]
    }

    fn confirm(&mut self, now: Duration, destination: SecretDestination) -> Vec<SecretEffect> {
        let confirmation = self
            .gate
            .pending(now, destination)
            .map(crate::secret::PendingSecretDelivery::confirmation_id);
        let authorization =
            confirmation.and_then(|pending_id| self.gate.confirm(pending_id, now, destination));
        let Some(authorization) = authorization else {
            self.phase = Phase::Idle;
            return vec![SecretEffect::ConsumeButton];
        };
        self.phase = Phase::Looking {
            destination: authorization.destination,
        };
        vec![
            SecretEffect::StartLookup(authorization),
            SecretEffect::ConsumeButton,
        ]
    }

    fn lookup_finished(
        &mut self,
        found: bool,
        now: Duration,
        current: Option<SecretDestination>,
    ) -> Vec<SecretEffect> {
        let Phase::Looking { destination } = self.phase else {
            return Vec::new();
        };
        self.phase = Phase::Idle;
        if !found {
            self.raise_notice(now);
            return Vec::new();
        }
        if current == Some(destination) {
            vec![SecretEffect::Deliver(destination)]
        } else {
            Vec::new()
        }
    }

    /// The overlay text while a confirmation is answerable.
    #[must_use]
    pub fn prompt(&self, now: Duration, current: Option<SecretDestination>) -> Option<&ConfirmationPrompt> {
        let Phase::Confirming { prompt, .. } = &self.phase else {
            return None;
        };
        let _pending = self.gate.pending(now, current?)?;
        Some(prompt)
    }

    /// Whether the "delivery unavailable" notice is showing.
    #[must_use]
    pub fn notice_active(&self, now: Duration) -> bool {
        self.notice_until.is_some_and(|until| now < until)
    }

    /// Whether a lookup is in flight.
    #[must_use]
    pub const fn looking(&self) -> bool {
        matches!(self.phase, Phase::Looking { .. })
    }
}

/// Keeps printable ASCII, replaces the rest, and shortens to `maximum`.
fn sanitize_label(value: &str, maximum: usize) -> String {
    let mut label: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .take(maximum)
        .collect();
    if value.chars().count() > maximum {
        label.push_str("...");
    }
    if label.is_empty() {
        label.push_str("unnamed");
    }
    label
}

impl Default for SecretFlow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CONFIRMATION_LIFETIME, NOTICE_LIFETIME, SecretEffect, SecretFlow, SecretInput};
    use crate::secret::SecretDestination;
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_protocol::{ButtonState, SecretReference};

    const DEST: SecretDestination = SecretDestination {
        surface: 1,
        focus_generation: 1,
    };
    const OTHER: SecretDestination = SecretDestination {
        surface: 1,
        focus_generation: 2,
    };
    const START: Duration = Duration::from_secs(100);

    const NONE: [SecretEffect; 0] = [];

    fn request() -> SecretInput {
        SecretInput::Request {
            reference: SecretReference { entry: "mail".into() },
            target: "Inbox".into(),
        }
    }

    fn step(flow: &mut SecretFlow, input: SecretInput) -> Vec<SecretEffect> {
        flow.step(input, START, Some(DEST))
    }

    fn feed(flow: &mut SecretFlow, input: SecretInput) {
        let _effects = step(flow, input);
    }

    fn started(effects: &[SecretEffect]) -> bool {
        effects
            .iter()
            .any(|effect| matches!(effect, SecretEffect::StartLookup(_)))
    }

    fn armed_flow() -> SecretFlow {
        let mut flow = SecretFlow::new();
        feed(&mut flow, request());
        feed(&mut flow, SecretInput::FramePresented);
        flow
    }

    #[test]
    fn press_after_presented_frame_starts_the_lookup() {
        let mut flow = armed_flow();
        let effects = step(&mut flow, SecretInput::Button(ButtonState::Pressed));
        assert!(started(&effects));
        assert!(effects.contains(&SecretEffect::ConsumeButton));
        assert!(flow.looking());
        assert!(flow.prompt(START, Some(DEST)).is_none());
    }

    #[test]
    fn press_before_the_frame_is_presented_does_not_confirm() {
        let mut flow = SecretFlow::new();
        feed(&mut flow, request());
        let effects = step(&mut flow, SecretInput::Button(ButtonState::Pressed));
        assert!(!started(&effects));
        assert_eq!(effects, [SecretEffect::ConsumeButton]);
        feed(&mut flow, SecretInput::FramePresented);
        assert!(!started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
    }

    #[test]
    fn button_held_when_the_frame_arrives_needs_a_release() {
        let mut flow = SecretFlow::new();
        feed(&mut flow, SecretInput::ButtonHeld(true));
        feed(&mut flow, request());
        feed(&mut flow, SecretInput::FramePresented);
        assert!(!started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
        feed(&mut flow, SecretInput::Button(ButtonState::Released));
        assert!(started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
    }

    #[test]
    fn release_before_the_frame_arms_on_the_frame() {
        let mut flow = SecretFlow::new();
        feed(&mut flow, SecretInput::ButtonHeld(true));
        feed(&mut flow, request());
        feed(&mut flow, SecretInput::Button(ButtonState::Released));
        feed(&mut flow, SecretInput::FramePresented);
        assert!(started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
    }

    #[test]
    fn a_confirmation_confirms_once() {
        let mut flow = armed_flow();
        assert!(started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
        feed(&mut flow, SecretInput::Button(ButtonState::Released));
        assert!(!started(&step(
            &mut flow,
            SecretInput::Button(ButtonState::Pressed)
        )));
    }

    #[test]
    fn buttons_pass_through_when_nothing_is_shown() {
        let mut flow = SecretFlow::new();
        assert_eq!(step(&mut flow, SecretInput::Button(ButtonState::Pressed)), NONE);
    }

    #[test]
    fn focus_change_cancels_the_confirmation() {
        let mut flow = armed_flow();
        feed(&mut flow, SecretInput::FocusChanged);
        let effects = flow.step(SecretInput::Button(ButtonState::Pressed), START, Some(OTHER));
        assert_eq!(effects, NONE);
        assert!(flow.prompt(START, Some(OTHER)).is_none());
    }

    #[test]
    fn a_press_from_another_destination_does_not_confirm() {
        let mut flow = armed_flow();
        let effects = flow.step(SecretInput::Button(ButtonState::Pressed), START, Some(OTHER));
        assert!(!started(&effects));
        assert!(
            flow.step(SecretInput::Button(ButtonState::Pressed), START, None)
                .is_empty()
        );
    }

    #[test]
    fn the_confirmation_expires() {
        let mut flow = armed_flow();
        let late = START.saturating_add(CONFIRMATION_LIFETIME);
        assert!(flow.prompt(late, Some(DEST)).is_none());
        let effects = flow.step(SecretInput::Button(ButtonState::Pressed), late, Some(DEST));
        assert!(!started(&effects));
    }

    #[test]
    fn prompt_names_the_entry_and_target_sanitised() {
        let mut flow = SecretFlow::new();
        let input = SecretInput::Request {
            reference: SecretReference {
                entry: "caf\u{e9}\n".into(),
            },
            target: "x".repeat(40),
        };
        feed(&mut flow, input);
        let prompt = flow.prompt(START, Some(DEST)).expect("prompt shown");
        assert_eq!(prompt.entry, "caf??");
        assert_eq!(prompt.target, format!("{}...", "x".repeat(29)));
    }

    #[test]
    fn request_without_focus_is_refused() {
        let mut flow = SecretFlow::new();
        assert_eq!(flow.step(request(), START, None), [SecretEffect::Refused]);
        assert!(flow.prompt(START, None).is_none());
    }

    #[test]
    fn request_during_a_lookup_is_refused() {
        let mut flow = armed_flow();
        feed(&mut flow, SecretInput::Button(ButtonState::Pressed));
        assert_eq!(step(&mut flow, request()), [SecretEffect::Refused]);
        assert!(flow.looking());
    }

    #[test]
    fn found_value_is_delivered_to_the_still_current_destination() {
        let mut flow = armed_flow();
        feed(&mut flow, SecretInput::Button(ButtonState::Pressed));
        let effects = step(&mut flow, SecretInput::LookupFinished { found: true });
        assert_eq!(effects, [SecretEffect::Deliver(DEST)]);
        assert!(!flow.looking());
    }

    #[test]
    fn found_value_is_dropped_when_focus_moved_during_the_lookup() {
        let mut flow = armed_flow();
        feed(&mut flow, SecretInput::Button(ButtonState::Pressed));
        feed(&mut flow, SecretInput::FocusChanged);
        let effects = flow.step(SecretInput::LookupFinished { found: true }, START, Some(OTHER));
        assert_eq!(effects, NONE);
        assert!(!flow.looking());
    }

    #[test]
    fn failed_lookup_raises_the_notice_for_a_short_time() {
        let mut flow = armed_flow();
        feed(&mut flow, SecretInput::Button(ButtonState::Pressed));
        assert_eq!(
            step(&mut flow, SecretInput::LookupFinished { found: false }),
            NONE
        );
        assert!(flow.notice_active(START));
        assert!(!flow.notice_active(START.saturating_add(NOTICE_LIFETIME)));
    }

    #[test]
    fn refused_request_and_failed_delivery_raise_the_notice() {
        let mut refused = SecretFlow::new();
        let _effects = refused.step(request(), START, None);
        assert!(refused.notice_active(START));
        let mut failed = SecretFlow::new();
        feed(&mut failed, SecretInput::DeliveryFailed);
        assert!(failed.notice_active(START));
    }

    #[test]
    fn lookup_result_without_a_lookup_is_ignored() {
        let mut flow = SecretFlow::new();
        assert_eq!(step(&mut flow, SecretInput::LookupFinished { found: true }), NONE);
    }

    #[derive(Debug, Clone)]
    enum Event {
        Request,
        Frame,
        Press,
        Release,
        Held(bool),
        Focus,
        Finish(bool),
        Wait(u64),
    }

    fn event() -> impl Strategy<Value = Event> {
        prop_oneof![
            Just(Event::Request),
            Just(Event::Frame),
            Just(Event::Press),
            Just(Event::Release),
            any::<bool>().prop_map(Event::Held),
            Just(Event::Focus),
            any::<bool>().prop_map(Event::Finish),
            (0_u64..15).prop_map(Event::Wait),
        ]
    }

    proptest! {
        /// A lookup starts only for a request made on the live destination
        /// whose prompt was presented, after the last release, before expiry,
        /// and each request starts at most one lookup.
        #[test]
        fn lookups_start_only_from_a_seen_unexpired_confirmed_request(
            events in proptest::collection::vec(event(), 0..60)
        ) {
            let mut flow = SecretFlow::new();
            let mut now = START;
            let mut generation = 1_u64;
            let mut requested: Option<(Duration, u64)> = None;
            let mut frame_seen = false;
            let mut starts_for_request = 0_u32;
            for event in events {
                let current = Some(SecretDestination { surface: 1, focus_generation: generation });
                let input = match event {
                    Event::Request => SecretInput::Request {
                        reference: SecretReference { entry: "mail".into() },
                        target: "Inbox".into(),
                    },
                    Event::Frame => SecretInput::FramePresented,
                    Event::Press => SecretInput::Button(ButtonState::Pressed),
                    Event::Release => SecretInput::Button(ButtonState::Released),
                    Event::Held(held) => SecretInput::ButtonHeld(held),
                    Event::Focus => {
                        generation = generation.saturating_add(1);
                        SecretInput::FocusChanged
                    }
                    Event::Finish(found) => SecretInput::LookupFinished { found },
                    Event::Wait(seconds) => {
                        now = now.saturating_add(Duration::from_secs(seconds));
                        continue;
                    }
                };
                let was_looking = flow.looking();
                let is_request = matches!(input, SecretInput::Request { .. });
                let is_frame = matches!(input, SecretInput::FramePresented);
                let current_before = current;
                let effects = flow.step(input, now, current_before);
                if is_request && !effects.contains(&SecretEffect::Refused) {
                    requested = Some((now, generation));
                    frame_seen = false;
                    starts_for_request = 0_u32;
                }
                if is_frame && flow.prompt(now, current_before).is_some() {
                    frame_seen = true;
                }
                if started(&effects) {
                    prop_assert!(!was_looking);
                    let (requested_at, made_in) = requested.expect("a request preceded the lookup");
                    prop_assert!(frame_seen);
                    prop_assert_eq!(made_in, generation);
                    prop_assert!(now < requested_at.saturating_add(CONFIRMATION_LIFETIME));
                    starts_for_request = starts_for_request.saturating_add(1);
                    prop_assert_eq!(starts_for_request, 1);
                }
                for effect in &effects {
                    if let SecretEffect::Deliver(destination) = effect {
                        prop_assert_eq!(Some(*destination), current_before);
                        prop_assert!(was_looking);
                    }
                }
            }
        }
    }
}

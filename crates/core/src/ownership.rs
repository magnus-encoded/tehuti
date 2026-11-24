//! Who gets a button press, as one pure answer (#800, step 1).
//!
//! This models today's routing exactly and is not wired into dispatch or the compositor yet.
//! The differential test at the bottom drives the existing routing and this module over generated
//! states and requires them to agree.
//!
//! The scope is what core `route` and the push-to-talk rules decide while no overlay other than
//! the system menu has the pad. Overlays (settings, launcher, keyboard, mouse mode, secret
//! confirmation) and the compositor's virtual-pad plugin overrides are later steps.

use tehuti_protocol::{ButtonName, GuideMode};

use crate::{Level, State, focused_context, global_context, pad};

/// Every pad button, so a view can be built from a state.
const ALL_BUTTONS: [ButtonName; 22] = [
    ButtonName::South,
    ButtonName::East,
    ButtonName::North,
    ButtonName::West,
    ButtonName::LeftBumper,
    ButtonName::RightBumper,
    ButtonName::LeftTrigger,
    ButtonName::RightTrigger,
    ButtonName::Select,
    ButtonName::Record,
    ButtonName::Start,
    ButtonName::Guide,
    ButtonName::LeftStick,
    ButtonName::RightStick,
    ButtonName::DpadUp,
    ButtonName::DpadDown,
    ButtonName::DpadLeft,
    ButtonName::DpadRight,
    ButtonName::Paddle1,
    ButtonName::Paddle2,
    ButtonName::Paddle3,
    ButtonName::Paddle4,
];

/// Whether a button goes down or comes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Pressed,
    Released,
}

/// Where the push-to-talk buttons come from, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkSource {
    /// The connected controller's own binding.
    Controller,
    /// The focused context's own `push_to_talk` list.
    Context,
    /// The global context's list.
    Global,
    /// Record, when nothing is configured.
    Default,
}

/// Which context a mapping comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Active,
    Global,
}

/// The reason for an owner, as a code; the compositor turns it into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Guide belongs to the escape chord.
    EscapeChord,
    /// A d-pad button pressed with Guide, and its release.
    GuideDpadChord,
    /// Start pressed with Guide held, and its release.
    ChordStart,
    /// The system menu is open.
    SystemMenu,
    /// The release of a button the system menu took.
    MenuRelease,
    /// LB and RB move columns at the columns level.
    ColumnsLevel,
    /// A push-to-talk button.
    PushToTalk,
    /// A plugin has the request.
    PluginRequest,
    /// A `[sticks]` modifier button.
    StickModifier,
    /// A `hold = true` mapping.
    HeldKey,
    /// A context's button mapping.
    ContextMapping,
    /// The global layer sends Enter for Confirm.
    GlobalEnter,
    /// Passthrough leaves the button to the application.
    Passthrough,
    /// Nothing takes the button.
    Unmapped,
}

/// Who gets a button, in precedence order (see `owner` for the order itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Tehuti(Why),
    Level,
    Talk(TalkSource),
    Plugin,
    Context { scope: Scope, why: Why },
    GlobalLayer,
    App,
    Dropped,
}

impl Owner {
    /// The code naming why this owner got the button.
    #[must_use]
    pub const fn why(self) -> Why {
        match self {
            Self::Tehuti(why) | Self::Context { why, .. } => why,
            Self::Level => Why::ColumnsLevel,
            Self::Talk(_) => Why::PushToTalk,
            Self::Plugin => Why::PluginRequest,
            Self::GlobalLayer => Why::GlobalEnter,
            Self::App => Why::Passthrough,
            Self::Dropped => Why::Unmapped,
        }
    }
}

/// A plain-data snapshot of what decides ownership, rebuilt from the state at the start of a step.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat snapshot of independent routing flags"
)]
pub struct OwnershipView {
    /// The button in the Confirm role on this pad.
    pub confirm: ButtonName,
    /// Passthrough is active: the focused context hands the pad to the application.
    pub passthrough: bool,
    pub level: Level,
    pub guide_mode: GuideMode,
    pub guide_held: bool,
    /// Start went down while Guide was held, so its release is the chord's too.
    pub start_in_chord: bool,
    /// D-pad buttons pressed with Guide held.
    pub chord_buttons: Vec<ButtonName>,
    pub menu_open: bool,
    /// Buttons held down now, as dispatch tracks them in passthrough; a talk list there needs every
    /// member held. `State` does not know them, so the caller fills this in.
    pub held_buttons: Vec<ButtonName>,
    /// Buttons the system menu took, whose release it consumes.
    pub menu_buttons: Vec<ButtonName>,
    /// The push-to-talk buttons and where they came from.
    pub talk: Vec<ButtonName>,
    pub talk_source: TalkSource,
    /// A plugin has claimed the buttons.
    pub plugin_active: bool,
    /// Buttons the focused context maps.
    pub focused_maps: Vec<ButtonName>,
    /// Buttons the global context maps.
    pub global_maps: Vec<ButtonName>,
    /// Buttons whose winning mapping is `hold = true`.
    pub holds: Vec<ButtonName>,
    /// Buttons the focused context names as `[sticks]` modifiers.
    pub stick_modifiers: Vec<ButtonName>,
    /// Stick modifiers currently held.
    pub sticks_held: Vec<ButtonName>,
}

impl OwnershipView {
    /// Whether Guide also reaches the application through the virtual pad. Tehuti still owns the
    /// press; this only says the compositor forwards it as well.
    #[must_use]
    pub fn guide_forwarded(&self) -> bool {
        self.passthrough && self.guide_mode != GuideMode::Tehuti
    }

    fn talks(&self, button: ButtonName) -> bool {
        self.talk.contains(&button)
    }

    /// Passthrough talk needs every member held; a Select-only list is special-cased instead.
    fn passthrough_talks(&self, button: ButtonName) -> bool {
        self.talks(button)
            && (self.talk == [ButtonName::Select]
                || self
                    .talk
                    .iter()
                    .all(|member| *member == button || self.held_buttons.contains(member)))
    }

    fn start_chord(&self, button: ButtonName, phase: Phase) -> bool {
        button == ButtonName::Start
            && match phase {
                Phase::Pressed => self.guide_held || self.start_in_chord,
                Phase::Released => self.start_in_chord,
            }
    }

    fn dpad_chord(&self, button: ButtonName) -> bool {
        matches!(
            button,
            ButtonName::DpadUp | ButtonName::DpadDown | ButtonName::DpadLeft | ButtonName::DpadRight
        ) && ((self.guide_held && self.guide_mode != GuideMode::App) || self.chord_buttons.contains(&button))
    }

    fn menu_release(&self, button: ButtonName) -> bool {
        !self.menu_open && self.menu_buttons.contains(&button)
    }

    /// The layers above push-to-talk: Guide, the Start chord and the open system menu.
    fn tehuti_first(&self, button: ButtonName, phase: Phase) -> Option<Owner> {
        if button == ButtonName::Guide {
            Some(Owner::Tehuti(Why::EscapeChord))
        } else if self.start_chord(button, phase) {
            Some(Owner::Tehuti(Why::ChordStart))
        } else if self.menu_open {
            Some(Owner::Tehuti(Why::SystemMenu))
        } else {
            None
        }
    }

    fn tehuti_after_talk(&self, button: ButtonName) -> Option<Owner> {
        if self.menu_release(button) {
            Some(Owner::Tehuti(Why::MenuRelease))
        } else if self.dpad_chord(button) {
            Some(Owner::Tehuti(Why::GuideDpadChord))
        } else {
            None
        }
    }

    fn maps(&self, button: ButtonName) -> Option<Scope> {
        if self.focused_maps.contains(&button) {
            Some(Scope::Active)
        } else if self.global_maps.contains(&button) {
            Some(Scope::Global)
        } else {
            None
        }
    }

    fn stick_layer(&self, button: ButtonName, phase: Phase) -> bool {
        match phase {
            Phase::Pressed => self.stick_modifiers.contains(&button),
            Phase::Released => self.sticks_held.contains(&button),
        }
    }

    /// The global layer, the stick modifiers, held keys, the plugin and the context mappings,
    /// in the order `route` tries them.
    fn route_layers(&self, button: ButtonName, phase: Phase) -> Owner {
        if phase == Phase::Pressed && button == self.confirm {
            return if self.focused_maps.contains(&button) {
                Owner::Context {
                    scope: Scope::Active,
                    why: Why::ContextMapping,
                }
            } else {
                Owner::GlobalLayer
            };
        }
        if self.stick_layer(button, phase) {
            return Owner::Context {
                scope: Scope::Active,
                why: Why::StickModifier,
            };
        }
        if phase == Phase::Pressed
            && !self.plugin_active
            && self.holds.contains(&button)
            && let Some(scope) = self.maps(button)
        {
            return Owner::Context {
                scope,
                why: Why::HeldKey,
            };
        }
        // Open question for the owner (#800): today a plugin outranks the focused context's own
        // mapping, not just the global context's. This keeps today's order.
        if self.plugin_active {
            return Owner::Plugin;
        }
        match (phase, self.maps(button)) {
            (Phase::Pressed, Some(scope)) => Owner::Context {
                scope,
                why: Why::ContextMapping,
            },
            (Phase::Pressed | Phase::Released, _) => Owner::Dropped,
        }
    }
}

/// Who gets `button` in `phase` right now.
///
/// Precedence, highest first. Without passthrough: Guide, the Start chord, the open system
/// menu, push-to-talk, a menu release, the Guide d-pad chord, the columns level, then the
/// route layers (global Confirm, stick modifier, held key, plugin, mapping, dropped). With
/// passthrough the chord layers come before push-to-talk and everything else goes to the app.
#[must_use]
pub fn owner(view: &OwnershipView, button: ButtonName, phase: Phase) -> Owner {
    if view.passthrough {
        return passthrough_owner(view, button, phase);
    }
    if let Some(owner) = view.tehuti_first(button, phase) {
        return owner;
    }
    if view.talks(button) {
        return Owner::Talk(view.talk_source);
    }
    if let Some(owner) = view.tehuti_after_talk(button) {
        return owner;
    }
    if view.level == Level::Columns && matches!(button, ButtonName::LeftBumper | ButtonName::RightBumper) {
        return Owner::Level;
    }
    view.route_layers(button, phase)
}

fn passthrough_owner(view: &OwnershipView, button: ButtonName, phase: Phase) -> Owner {
    if view.menu_release(button) {
        return Owner::Tehuti(Why::MenuRelease);
    }
    if let Some(owner) = view.tehuti_first(button, phase) {
        return owner;
    }
    // Open question for the owner (#800): a Select-only talk list is special-cased in dispatch
    // (a tap talks, a hold takes the pad back). It stays Talk here, as today.
    // Talk comes before the Guide d-pad chord, as in normal mode. Today a d-pad talk member
    // pressed with Guide held both talks and chords; one owner cannot say that, so wiring this
    // in (step 5) removes the double fire.
    if view.passthrough_talks(button) {
        return Owner::Talk(view.talk_source);
    }
    if view.dpad_chord(button) {
        return Owner::Tehuti(Why::GuideDpadChord);
    }
    Owner::App
}

/// The owner each held button had at its press, so a release goes where its press went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    held: Vec<(ButtonName, Owner)>,
}

impl Claims {
    /// Claims a press; a repeated press keeps its first owner.
    pub fn press(&mut self, view: &OwnershipView, button: ButtonName) -> Owner {
        if let Some((_, claimed)) = self.held.iter().find(|(held, _)| *held == button) {
            return *claimed;
        }
        let claimed = owner(view, button, Phase::Pressed);
        self.held.push((button, claimed));
        claimed
    }

    /// The owner of the press a release answers; a release without a press is dropped.
    pub fn release(&mut self, button: ButtonName) -> Owner {
        self.held
            .iter()
            .position(|(held, _)| *held == button)
            .map_or(Owner::Dropped, |index| self.held.remove(index).1)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

impl State {
    /// The ownership snapshot of this state; rebuild it each step, never cache it.
    #[must_use]
    pub fn ownership_view(&self, plugin_active: bool) -> OwnershipView {
        let focused = focused_context(self);
        let global = global_context(self);
        let passthrough = self.passthrough();
        let (talk, talk_source) = self.talk_buttons();
        let talk = talk.to_vec();
        let maps = |context: Option<&tehuti_protocol::Context>| -> Vec<ButtonName> {
            context.map_or_else(Vec::new, |context| context.mapped_buttons().collect())
        };
        let holds = ALL_BUTTONS
            .into_iter()
            .filter(|button| {
                focused
                    .filter(|context| context.action_for(*button).is_some())
                    .or(global)
                    .is_some_and(|context| context.held_key_for(*button).is_some())
            })
            .collect();
        OwnershipView {
            confirm: self.pad_family.button(pad::Role::Confirm),
            passthrough,
            level: self.level,
            guide_mode: self.guide_mode(),
            guide_held: self.guide_held.is_some(),
            start_in_chord: self.start_in_chord,
            chord_buttons: self.chord_buttons.clone(),
            menu_open: self.menu.is_some(),
            held_buttons: Vec::new(),
            menu_buttons: self.menu_buttons.clone(),
            talk,
            talk_source,
            plugin_active,
            focused_maps: maps(focused),
            global_maps: maps(global),
            holds,
            stick_modifiers: ALL_BUTTONS
                .into_iter()
                .filter(|button| focused.is_some_and(|context| context.stick_modifier(*button)))
                .collect(),
            sticks_held: ALL_BUTTONS
                .into_iter()
                .filter(|button| self.sticks.holds(*button))
                .collect(),
        }
    }

    /// Who gets `button` on the virtual pad, from the mirror's own held list. The mirror sees raw
    /// events before the core does, so Guide, the held buttons and the chord Start come from it.
    #[must_use]
    pub fn pad_button_owner(
        &self,
        held: &[ButtonName],
        start_in_chord: bool,
        button: ButtonName,
        phase: Phase,
    ) -> Owner {
        let mut view = self.ownership_view(false);
        view.passthrough = true;
        view.guide_held = held.contains(&ButtonName::Guide);
        view.start_in_chord = start_in_chord;
        view.held_buttons = held.to_vec();
        owner(&view, button, phase)
    }
}

impl Owner {
    /// Hides Guide unless forwarded, chord Start, and a winning Talk binding.
    #[must_use]
    pub const fn hides_from_pad(self, guide_forwarded: bool) -> bool {
        matches!(self, Self::Tehuti(Why::ChordStart) | Self::Talk(_))
            || (matches!(self, Self::Tehuti(Why::EscapeChord)) && !guide_forwarded)
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_protocol::{Action, ButtonState, Event, KeyName, parse_context};

    use super::*;
    use crate::{escape_chord, menu_button_release, pad::PadFamily, step_with_plugin};

    fn any_button() -> impl Strategy<Value = ButtonName> {
        prop::sample::select(ALL_BUTTONS.to_vec())
    }

    fn any_phase() -> impl Strategy<Value = Phase> {
        prop_oneof![Just(Phase::Pressed), Just(Phase::Released)]
    }

    fn buttons() -> impl Strategy<Value = Vec<ButtonName>> {
        prop::collection::vec(any_button(), 0..5)
    }

    fn any_level() -> impl Strategy<Value = Level> {
        prop_oneof![Just(Level::App), Just(Level::Columns)]
    }

    fn any_guide_mode() -> impl Strategy<Value = GuideMode> {
        prop_oneof![
            Just(GuideMode::Tehuti),
            Just(GuideMode::Both),
            Just(GuideMode::App)
        ]
    }

    fn any_source() -> impl Strategy<Value = TalkSource> {
        prop_oneof![
            Just(TalkSource::Controller),
            Just(TalkSource::Context),
            Just(TalkSource::Global),
            Just(TalkSource::Default)
        ]
    }

    fn any_view() -> impl Strategy<Value = OwnershipView> {
        let flags = (
            any::<bool>(),
            any_level(),
            any_guide_mode(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        );
        let lists = (
            buttons(),
            buttons(),
            buttons(),
            buttons(),
            buttons(),
            buttons(),
            buttons(),
        );
        (flags, lists, any_source(), any_button()).prop_map(
            |(
                (passthrough, level, guide_mode, guide_held, start_in_chord, menu_open, plugin_active),
                (chord_buttons, menu_buttons, talk, focused_maps, global_maps, holds, stick_modifiers),
                talk_source,
                confirm,
            )| OwnershipView {
                confirm,
                passthrough,
                level,
                guide_mode,
                guide_held,
                start_in_chord,
                held_buttons: chord_buttons.clone(),
                chord_buttons,
                menu_open,
                menu_buttons: menu_buttons
                    .into_iter()
                    .filter(|held| !matches!(held, ButtonName::Guide | ButtonName::Start))
                    .collect(),
                talk,
                talk_source,
                plugin_active,
                focused_maps,
                global_maps,
                holds,
                sticks_held: stick_modifiers.clone(),
                stick_modifiers,
            },
        )
    }

    proptest! {
        /// 1. Total: every button and phase has exactly one owner, the same each time.
        #[test]
        fn every_button_and_phase_has_one_owner(view in any_view()) {
            for button in ALL_BUTTONS {
                for phase in [Phase::Pressed, Phase::Released] {
                    let first = owner(&view, button, phase);
                    prop_assert_eq!(first, owner(&view, button, phase));
                    prop_assert_eq!(first.why() == Why::Passthrough, first == Owner::App);
                }
            }
        }

        /// 2. Precedence: a higher layer that applies leaves no lower owner.
        #[test]
        fn a_higher_layer_leaves_no_lower_owner(view in any_view(), button in any_button(), phase in any_phase()) {
            let got = owner(&view, button, phase);
            if view.passthrough {
                prop_assert!(matches!(got, Owner::Tehuti(_) | Owner::Talk(_) | Owner::App));
            }
            if !view.passthrough && view.menu_open {
                prop_assert!(matches!(got, Owner::Tehuti(_)));
            }
            if !view.passthrough
                && !view.menu_open
                && view.talks(button)
                && button != ButtonName::Guide
                && !view.start_chord(button, phase)
            {
                prop_assert_eq!(got, Owner::Talk(view.talk_source));
            }
            if !view.passthrough && !matches!(got, Owner::Tehuti(_) | Owner::Talk(_)) {
                prop_assert!(!view.dpad_chord(button) && !view.menu_release(button));
            }
            if view.level == Level::Columns
                && !view.passthrough
                && matches!(got, Owner::Plugin | Owner::Context { .. } | Owner::GlobalLayer | Owner::Dropped)
            {
                prop_assert!(!matches!(button, ButtonName::LeftBumper | ButtonName::RightBumper));
            }
        }

        /// 3. Release symmetry: a release goes to its press's owner whatever changed between.
        #[test]
        fn a_release_goes_to_its_presses_owner(before in any_view(), after in any_view(), button in any_button()) {
            let mut claims = Claims::default();
            let pressed = claims.press(&before, button);
            prop_assert_eq!(claims.press(&after, button), pressed);
            prop_assert_eq!(claims.release(button), pressed);
            prop_assert_eq!(claims.release(button), Owner::Dropped);
        }

        /// 4. No stuck: once everything is released the claims are empty, and a release without
        /// a press never reaches the app.
        #[test]
        fn no_claim_outlives_its_release(
            view in any_view(),
            presses in prop::collection::vec(any_button(), 0..12),
            stray in any_button(),
        ) {
            let mut claims = Claims::default();
            for button in &presses {
                let _owner = claims.press(&view, *button);
            }
            for button in &presses {
                let _owner = claims.release(*button);
            }
            prop_assert!(claims.is_empty());
            prop_assert_eq!(claims.release(stray), Owner::Dropped);
        }

        /// 5. Guide: the escape chord always owns it, in every mode, phase and level.
        #[test]
        fn the_escape_chord_always_owns_guide(view in any_view(), phase in any_phase()) {
            prop_assert_eq!(owner(&view, ButtonName::Guide, phase), Owner::Tehuti(Why::EscapeChord));
            prop_assert!(!view.guide_forwarded() || (view.passthrough && view.guide_mode != GuideMode::Tehuti));
        }

        /// 7. Chord: Start with Guide down and its release are never the app's, in any mode.
        #[test]
        fn chord_start_is_never_the_apps(view in any_view()) {
            let mut view = view;
            view.guide_held = true;
            prop_assert_eq!(owner(&view, ButtonName::Start, Phase::Pressed), Owner::Tehuti(Why::ChordStart));
            view.start_in_chord = true;
            view.guide_held = false;
            prop_assert_eq!(owner(&view, ButtonName::Start, Phase::Released), Owner::Tehuti(Why::ChordStart));
        }
    }

    const FOCUSED: &str = "app";

    fn context_source(app_id: &str, scope: Scope, maps: &[(ButtonName, bool)], pad_section: &str) -> String {
        let (text, key) = match scope {
            Scope::Active => ("f", "x"),
            Scope::Global => ("g", "y"),
        };
        let mut source = format!("app_id = '{app_id}'\n");
        if scope == Scope::Global {
            source.push_str(pad_section);
        }
        source.push_str("[buttons]\n");
        for (button, hold) in maps {
            let name = toml::Value::try_from(button).unwrap();
            let name = name.as_str().unwrap();
            if *hold {
                writeln!(source, "{name} = {{ key = '{key}', hold = true }}").unwrap();
            } else {
                writeln!(source, "{name} = {{ text = '{text}' }}").unwrap();
            }
        }
        if scope == Scope::Active {
            source.push_str(pad_section);
        }
        source
    }

    fn list(buttons: &[ButtonName]) -> String {
        let names: Vec<String> = buttons
            .iter()
            .map(|button| format!("'{}'", toml::Value::try_from(button).unwrap().as_str().unwrap()))
            .collect();
        format!("[{}]", names.join(", "))
    }

    /// A generated state with the knobs routing reads set, and the event to route.
    #[derive(Debug, Clone)]
    #[expect(
        clippy::struct_excessive_bools,
        reason = "a flat generated snapshot of routing flags"
    )]
    struct Scenario {
        family: PadFamily,
        passthrough: bool,
        guide: GuideMode,
        focused: Vec<(ButtonName, bool)>,
        global: Vec<(ButtonName, bool)>,
        own_talk: Option<Vec<ButtonName>>,
        global_talk: Option<Vec<ButtonName>>,
        controller_talk: Option<ButtonName>,
        stick_with: Option<ButtonName>,
        stick_held: bool,
        level: Level,
        guide_held: bool,
        start_in_chord: bool,
        chord_buttons: Vec<ButtonName>,
        menu_open: bool,
        menu_buttons: Vec<ButtonName>,
        held: Vec<ButtonName>,
        plugin_active: bool,
        button: ButtonName,
        phase: Phase,
    }

    fn mappings() -> impl Strategy<Value = Vec<(ButtonName, bool)>> {
        prop::collection::vec((any_button(), any::<bool>()), 0..8).prop_map(|mut maps| {
            let mut seen = Vec::new();
            maps.retain(|(button, _)| {
                let fresh = !seen.contains(button);
                seen.push(*button);
                fresh
            });
            maps
        })
    }

    fn dpad() -> impl Strategy<Value = ButtonName> {
        prop::sample::select(vec![
            ButtonName::DpadUp,
            ButtonName::DpadDown,
            ButtonName::DpadLeft,
            ButtonName::DpadRight,
        ])
    }

    /// A talk list the parser accepts: buttons a single entry may name, and not empty.
    fn usable_talk(talk: Vec<ButtonName>) -> Option<Vec<ButtonName>> {
        let mut seen = Vec::new();
        let talk: Vec<ButtonName> = talk
            .into_iter()
            .filter(|button| {
                let fresh = !seen.contains(button);
                seen.push(*button);
                fresh
            })
            .filter(|button| {
                matches!(
                    button,
                    ButtonName::Record
                        | ButtonName::Select
                        | ButtonName::Paddle1
                        | ButtonName::Paddle2
                        | ButtonName::Paddle3
                        | ButtonName::Paddle4
                )
            })
            .collect();
        (!talk.is_empty()).then_some(talk)
    }

    #[expect(clippy::too_many_lines, reason = "one flat generator for every routing knob")]
    fn scenario() -> impl Strategy<Value = Scenario> {
        let setup = (
            prop::sample::select(vec![PadFamily::Letters, PadFamily::Nintendo]),
            any::<bool>(),
            any_guide_mode(),
            mappings(),
            mappings(),
            prop::option::of(buttons()),
            prop::option::of(buttons()),
            prop::option::of(any_button()),
        );
        let state = (
            prop::option::of(prop::sample::select(vec![
                ButtonName::South,
                ButtonName::East,
                ButtonName::North,
                ButtonName::West,
            ])),
            any::<bool>(),
            any_level(),
            any::<bool>(),
            any::<bool>(),
            prop::collection::vec(dpad(), 0..3),
            any::<bool>(),
            prop::collection::vec(any_button(), 0..3),
            buttons(),
        );
        (setup, state, any::<bool>(), any_button(), any_phase()).prop_map(
            |(
                (family, passthrough, guide, focused, global, own_talk, global_talk, controller_talk),
                (
                    stick_with,
                    stick_held,
                    level,
                    guide_held,
                    start_in_chord,
                    chord_buttons,
                    menu_open,
                    menu_buttons,
                    held,
                ),
                plugin_active,
                button,
                phase,
            )| Scenario {
                family,
                passthrough,
                guide,
                focused,
                global,
                own_talk: own_talk.and_then(usable_talk),
                global_talk: global_talk.and_then(usable_talk),
                controller_talk,
                stick_with,
                stick_held,
                level,
                guide_held,
                start_in_chord,
                chord_buttons,
                menu_open,
                menu_buttons: menu_buttons
                    .into_iter()
                    .filter(|pressed| !matches!(pressed, ButtonName::Guide | ButtonName::Start))
                    .collect(),
                held,
                plugin_active,
                button,
                phase,
            },
        )
    }

    impl Scenario {
        fn state(&self) -> State {
            let mut pad = String::new();
            if self.passthrough {
                pad.push_str("[pad]\npassthrough = true\nvirtual = true\n");
                let mode = match self.guide {
                    GuideMode::Tehuti => "tehuti",
                    GuideMode::Both => "both",
                    GuideMode::App => "app",
                };
                writeln!(pad, "guide = '{mode}'").unwrap();
            } else if self.own_talk.is_some() {
                pad.push_str("[pad]\n");
            }
            if let Some(own) = &self.own_talk {
                writeln!(pad, "push_to_talk = {}", list(own)).unwrap();
            }
            let mut focused = context_source(FOCUSED, Scope::Active, &self.focused, &pad);
            if let Some(with) = self.stick_with {
                let name = toml::Value::try_from(with).unwrap();
                writeln!(
                    focused,
                    "[sticks]\nleft.up = {{ text = 's', with = '{}' }}",
                    name.as_str().unwrap()
                )
                .unwrap();
            }
            let top = self
                .global_talk
                .as_ref()
                .map_or_else(String::new, |talk| format!("push_to_talk = {}\n", list(talk)));
            let global = context_source("*", Scope::Global, &self.global, &top);
            let mut state = State::with_contexts(vec![
                parse_context(&focused).unwrap(),
                parse_context(&global).unwrap(),
            ]);
            state.set_pad_family(self.family);
            state.set_controller_push_to_talk(self.controller_talk);
            let (mut state, _) = crate::step(
                state,
                &Event::Focus {
                    app_id: Some(FOCUSED.into()),
                    title: String::new(),
                },
            );
            state.level = self.level;
            state.guide_held = self.guide_held.then_some(Duration::ZERO);
            state.start_in_chord = self.start_in_chord;
            state.chord_buttons.clone_from(&self.chord_buttons);
            state.menu_buttons.clone_from(&self.menu_buttons);
            if self.menu_open {
                state.menu = Some(state.new_menu());
            }
            if self.stick_held
                && let Some(with) = self.stick_with
            {
                state.sticks.press_modifier(with);
            }
            state
        }

        fn event(&self) -> Event {
            Event::Button {
                name: self.button,
                state: match self.phase {
                    Phase::Pressed => ButtonState::Pressed,
                    Phase::Released => ButtonState::Released,
                },
            }
        }
    }

    /// What the old routing did with the event, as an owner kind.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Tehuti,
        Level,
        Talk,
        Plugin,
        Context(Scope),
        GlobalLayer,
        App,
        Dropped,
        /// Behaviour of the stick-modifier layer, which may replay a tap as a press.
        Sticks,
    }

    fn kind(owner: Owner) -> Kind {
        match owner {
            Owner::Tehuti(_) => Kind::Tehuti,
            Owner::Level => Kind::Level,
            Owner::Talk(_) => Kind::Talk,
            Owner::Plugin => Kind::Plugin,
            Owner::Context {
                why: Why::StickModifier,
                ..
            } => Kind::Sticks,
            Owner::Context { scope, .. } => Kind::Context(scope),
            Owner::GlobalLayer => Kind::GlobalLayer,
            Owner::App => Kind::App,
            Owner::Dropped => Kind::Dropped,
        }
    }

    /// The existing routing's answer: its own predicates for the Tehuti and talk layers, then the
    /// actions `route` returns for everything it routes itself.
    fn old_routing(scenario: &Scenario, state: &State) -> Kind {
        let event = scenario.event();
        let chord = escape_chord(&mut state.clone(), &event).is_some();
        let keyed_chord = chord && matches!(scenario.button, ButtonName::Guide | ButtonName::Start);
        let menu_release = menu_button_release(&mut state.clone(), &event);
        let first = keyed_chord || state.menu.is_some();
        if state.passthrough() {
            if menu_release || first {
                return Kind::Tehuti;
            }
            // Dispatch fires talk from the held buttons before `route` reaches the chord.
            let members = state.talk_buttons().0;
            let held = |member: &ButtonName| *member == scenario.button || scenario.held.contains(member);
            if members.contains(&scenario.button)
                && (members == [ButtonName::Select] || members.iter().all(held))
            {
                return Kind::Talk;
            }
            return if chord { Kind::Tehuti } else { Kind::App };
        }
        if first {
            return Kind::Tehuti;
        }
        if state.push_to_talk(scenario.button) {
            return Kind::Talk;
        }
        if menu_release || chord {
            return Kind::Tehuti;
        }
        let (_, actions, routed) = step_with_plugin(state.clone(), &event, scenario.plugin_active);
        let shoulder = matches!(scenario.button, ButtonName::LeftBumper | ButtonName::RightBumper);
        if shoulder && state.level == Level::Columns {
            return Kind::Level;
        }
        // The global layer takes an unmodified Confirm press before the stick modifiers see it.
        let confirm_press = scenario.phase == Phase::Pressed
            && scenario.button == state.pad_family.button(pad::Role::Confirm);
        if !confirm_press
            && (match scenario.phase {
                Phase::Pressed => state.focused_stick_modifier(scenario.button),
                Phase::Released => state.sticks.holds(scenario.button),
            })
        {
            return Kind::Sticks;
        }
        if routed {
            return Kind::Plugin;
        }
        match actions.as_slice() {
            [Action::Layout { .. }] if shoulder => Kind::Level,
            [Action::Text { text }] if text.as_str() == "f" => Kind::Context(Scope::Active),
            [Action::Text { text }] if text.as_str() == "g" => Kind::Context(Scope::Global),
            [Action::KeyDown { key: KeyName::X }] => Kind::Context(Scope::Active),
            [Action::KeyDown { key: KeyName::Y }] => Kind::Context(Scope::Global),
            [
                Action::Key {
                    key: KeyName::Enter, ..
                },
            ] => Kind::GlobalLayer,
            _ => Kind::Dropped,
        }
    }

    impl State {
        fn focused_stick_modifier(&self, button: ButtonName) -> bool {
            focused_context(self).is_some_and(|context| context.stick_modifier(button))
        }
    }

    proptest! {
        /// Differential: `owner` agrees with today's routing on generated states and buttons.
        #[test]
        fn owner_agrees_with_todays_routing(scenario in scenario()) {
            let state = scenario.state();
            let mut view = state.ownership_view(scenario.plugin_active);
            view.held_buttons.clone_from(&scenario.held);
            let got = owner(&view, scenario.button, scenario.phase);
            prop_assert_eq!(kind(got), old_routing(&scenario, &state), "view {:?}", view);
        }

        /// The view's push-to-talk set is what `push_to_talk` says, in every mode.
        #[test]
        fn the_view_talks_like_the_old_lookups(scenario in scenario(), button in any_button()) {
            let state = scenario.state();
            let view = state.ownership_view(false);
            prop_assert_eq!(view.talks(button), state.push_to_talk(button));
        }
    }

    #[test]
    fn a_dpad_talk_button_talks_in_passthrough_even_with_guide_held() {
        let mut state = State::with_contexts(vec![
            parse_context("app_id = 'app'\n[buttons]\n[pad]\npassthrough = true").unwrap(),
        ]);
        state.set_controller_push_to_talk(Some(ButtonName::DpadUp));
        let (mut state, _) = crate::step(
            state,
            &Event::Focus {
                app_id: Some("app".into()),
                title: String::new(),
            },
        );
        state.guide_held = Some(Duration::ZERO);
        let view = state.ownership_view(false);
        assert_eq!(
            owner(&view, ButtonName::DpadUp, Phase::Pressed),
            Owner::Talk(TalkSource::Controller)
        );
        assert_eq!(
            owner(&view, ButtonName::DpadDown, Phase::Pressed),
            Owner::Tehuti(Why::GuideDpadChord)
        );
    }

    #[test]
    fn passthrough_talk_needs_every_member_held() {
        let mut view = State::default().ownership_view(false);
        view.passthrough = true;
        view.talk = vec![ButtonName::Record, ButtonName::LeftBumper];
        assert_eq!(owner(&view, ButtonName::Record, Phase::Pressed), Owner::App);
        view.held_buttons = vec![ButtonName::LeftBumper];
        assert_eq!(
            owner(&view, ButtonName::Record, Phase::Pressed),
            Owner::Talk(view.talk_source)
        );
    }

    #[test]
    fn the_talk_source_follows_the_lookup_order() {
        let global = "app_id = '*'\npush_to_talk = ['start']\n[buttons]";
        let own = "app_id = 'app'\n[buttons]\n[pad]\npush_to_talk = ['select']";
        let contexts = || vec![parse_context(own).unwrap(), parse_context(global).unwrap()];
        let focus = Event::Focus {
            app_id: Some("app".into()),
            title: String::new(),
        };
        let (mut state, _) = crate::step(State::with_contexts(contexts()), &focus);
        let view1 = state.ownership_view(false);
        assert_eq!(
            (view1.talk, view1.talk_source),
            (vec![ButtonName::Select], TalkSource::Context)
        );
        state.set_controller_push_to_talk(Some(ButtonName::East));
        let view2 = state.ownership_view(false);
        assert_eq!(
            (view2.talk, view2.talk_source),
            (vec![ButtonName::East], TalkSource::Controller)
        );
        let (other, _) = crate::step(
            State::with_contexts(vec![parse_context(global).unwrap()]),
            &Event::Focus {
                app_id: Some("other".into()),
                title: String::new(),
            },
        );
        let view3 = other.ownership_view(false);
        assert_eq!(
            (view3.talk, view3.talk_source),
            (vec![ButtonName::Start], TalkSource::Global)
        );
        let view4 = State::default().ownership_view(false);
        assert_eq!(
            (view4.talk, view4.talk_source),
            (vec![ButtonName::Record], TalkSource::Default)
        );
    }

    #[test]
    fn guide_is_forwarded_only_through_a_passthrough_pad_that_asks() {
        let mut view = State::default().ownership_view(false);
        view.guide_mode = GuideMode::Both;
        assert!(
            !view.guide_forwarded(),
            "Both without passthrough stays with Tehuti"
        );
        view.passthrough = true;
        assert!(view.guide_forwarded());
        assert_eq!(
            owner(&view, ButtonName::Guide, Phase::Pressed),
            Owner::Tehuti(Why::EscapeChord)
        );
    }

    proptest! {
        /// The virtual pad hides chord Start across generated configurations and held buttons.
        #[test]
        fn the_pad_hook_hides_what_tehuti_owns(forwarded in any::<bool>(), others in buttons(), scenario in scenario()) {
            let state = scenario.state();
            let held: Vec<ButtonName> = others
                .into_iter()
                .filter(|held| !matches!(held, ButtonName::Guide | ButtonName::Start | ButtonName::Record))
                .chain([ButtonName::Guide, ButtonName::Start])
                .collect();
            let start = state.pad_button_owner(&held, false, ButtonName::Start, Phase::Pressed);
            prop_assert_eq!(start, Owner::Tehuti(Why::ChordStart));
            prop_assert!(start.hides_from_pad(forwarded));
            let release = state.pad_button_owner(&[ButtonName::Guide], true, ButtonName::Start, Phase::Released);
            prop_assert!(release.hides_from_pad(forwarded));
        }
    }

    #[test]
    fn the_pad_hook_talks_only_once_every_member_is_held() {
        let mut state = State::default();
        state.set_controller_push_to_talk(Some(ButtonName::Record));
        let talk = state.pad_button_owner(&[ButtonName::Record], false, ButtonName::Record, Phase::Pressed);
        assert_eq!(talk, Owner::Talk(TalkSource::Controller));
        assert!(talk.hides_from_pad(false));
    }
}

//! Text commits for the compositor's seat, without an external input method.

use smithay::reexports::{
    wayland_protocols::wp::text_input::zv3::server::{
        zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
        zwp_text_input_v3::{self, ZwpTextInputV3},
    },
    wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        protocol::wl_surface::WlSurface,
    },
};

use crate::state::Compositor;
use tehuti_core::keyboard::FieldKind;

#[derive(Debug, Default)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility satisfies unreachable_pub for state shared with the sibling module"
)]
pub(crate) struct TextInput {
    focus: Option<WlSurface>,
    instances: Vec<Instance>,
}

#[derive(Debug, Default)]
enum SurroundingUpdate {
    #[default]
    Unchanged,
    Reported(Option<(String, usize, usize)>),
}

#[derive(Debug)]
struct Instance {
    resource: ZwpTextInputV3,
    pending_enabled: Option<bool>,
    enabled: bool,
    serial: u32,
    kind: FieldKind,
    pending_kind: Option<FieldKind>,
    surrounding: Option<(String, usize, usize)>,
    pending_surrounding: SurroundingUpdate,
}

impl Instance {
    fn set_surrounding(&mut self, text: String, cursor: i32, anchor: i32) {
        self.pending_surrounding = SurroundingUpdate::Reported(
            usize::try_from(cursor)
                .ok()
                .zip(usize::try_from(anchor).ok())
                .filter(|(cursor, anchor)| {
                    text.len() <= 4000 && text.is_char_boundary(*cursor) && text.is_char_boundary(*anchor)
                })
                .map(|(cursor, anchor)| (text, cursor, anchor)),
        );
    }

    fn commit(&mut self, focused: bool) -> bool {
        self.serial = self.serial.wrapping_add(1);
        let changed = self.pending_enabled == Some(true);
        if changed {
            self.surrounding = None;
        }
        if let SurroundingUpdate::Reported(surrounding) = core::mem::take(&mut self.pending_surrounding) {
            self.surrounding = surrounding;
        }
        if let Some(kind) = self.pending_kind.take() {
            self.kind = kind;
        }
        if let Some(enabled) = self.pending_enabled.take() {
            self.enabled = enabled && focused;
        }
        changed
    }
}

impl TextInput {
    fn resource_status(&self, resource: &ZwpTextInputV3) -> (bool, bool) {
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|surface| resource.id().same_client_as(&surface.id()));
        let another_enabled = self
            .instances
            .iter()
            .any(|instance| instance.resource != *resource && instance.enabled);
        (focused, another_enabled)
    }

    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let _global = display.create_global::<Compositor, ZwpTextInputManagerV3, _>(1, ());
        Self::default()
    }

    pub(crate) fn focus(&mut self, focus: Option<WlSurface>) {
        for instance in &mut self.instances {
            if let Some(old) = &self.focus
                && instance.resource.id().same_client_as(&old.id())
            {
                instance.resource.leave(old);
            }
            instance.enabled = false;
            instance.pending_enabled = None;
            instance.kind = FieldKind::Normal;
            instance.pending_kind = None;
            instance.surrounding = None;
            instance.pending_surrounding = SurroundingUpdate::Unchanged;
            if let Some(surface) = &focus
                && instance.resource.id().same_client_as(&surface.id())
            {
                instance.resource.enter(surface);
            }
        }
        self.focus = focus;
    }

    pub(crate) fn edit_keyboard(
        &self,
        previous: &tehuti_core::keyboard::Keyboard,
        current: &tehuti_core::keyboard::Keyboard,
    ) -> usize {
        let old = previous.draft();
        let new = current.draft();
        let position = previous.caret();
        let anchor = previous.application_anchor().unwrap_or(position);
        let selection = position.min(anchor)..position.max(anchor);
        let prefix = old
            .chars()
            .zip(new.chars())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum::<usize>()
            .min(selection.start);
        let suffix = old
            .get(prefix..)
            .unwrap_or_default()
            .chars()
            .rev()
            .zip(new.get(prefix..).unwrap_or_default().chars().rev())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum::<usize>()
            .min(old.len().saturating_sub(selection.end));
        let end = new.len().saturating_sub(suffix);
        let Some(text) = new.get(prefix..end) else {
            return position;
        };
        self.edit(
            selection.start.saturating_sub(prefix),
            old.len().saturating_sub(selection.end).saturating_sub(suffix),
            text,
        );
        end
    }

    pub(crate) fn edit(&self, before: usize, after: usize, text: &str) {
        let (Ok(before), Ok(after)) = (u32::try_from(before), u32::try_from(after)) else {
            return;
        };
        for instance in &self.instances {
            if instance.enabled
                && self
                    .focus
                    .as_ref()
                    .is_some_and(|surface| instance.resource.id().same_client_as(&surface.id()))
            {
                instance.resource.delete_surrounding_text(before, after);
                if text.is_empty() {
                    instance.resource.commit_string(Some(String::new()));
                    instance.resource.done(instance.serial);
                }
            }
        }
        let _sent = self.send(text);
    }

    pub(crate) fn send(&self, text: &str) -> bool {
        let Some(surface) = self.focus.as_ref().filter(|surface| surface.is_alive()) else {
            return false;
        };
        let mut sent = false;
        for instance in &self.instances {
            if instance.enabled && instance.resource.id().same_client_as(&surface.id()) {
                sent = true;
                // https://wayland.app/protocols/text-input-unstable-v3: strings fit in 4000 bytes.
                let mut remaining = text;
                while !remaining.is_empty() {
                    let end = remaining.floor_char_boundary(remaining.len().min(4000));
                    let Some((chunk, rest)) = remaining.split_at_checked(end) else {
                        return true;
                    };
                    instance.resource.commit_string(Some(chunk.into()));
                    instance.resource.done(instance.serial);
                    remaining = rest;
                }
            }
        }
        sent
    }
}

impl GlobalDispatch<ZwpTextInputManagerV3, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTextInputManagerV3>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let _manager = data_init.init(resource, ());
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_text_input_manager_v3::Request::GetTextInput { id: input_id, .. } = request {
            let resource = data_init.init(input_id, ());
            if let Some(surface) = &state.text_input.focus
                && resource.id().same_client_as(&surface.id())
            {
                resource.enter(surface);
            }
            state.text_input.instances.push(Instance {
                resource,
                pending_enabled: None,
                enabled: false,
                serial: 0,
                kind: FieldKind::Normal,
                pending_kind: None,
                surrounding: None,
                pending_surrounding: SurroundingUpdate::Unchanged,
            });
        }
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let (focused, another_enabled) = state.text_input.resource_status(resource);
        let mut field = None;
        if let Some(instance) = state
            .text_input
            .instances
            .iter_mut()
            .find(|instance| instance.resource == *resource)
        {
            match request {
                zwp_text_input_v3::Request::Enable if focused && !another_enabled => {
                    instance.pending_enabled = Some(true);
                    instance.pending_kind = Some(FieldKind::Normal);
                }
                zwp_text_input_v3::Request::Disable => instance.pending_enabled = Some(false),
                zwp_text_input_v3::Request::Commit => {
                    let surrounding_changed =
                        matches!(instance.pending_surrounding, SurroundingUpdate::Reported(_))
                            || instance.pending_enabled == Some(true);
                    let changed = instance.commit(focused);
                    if focused && !another_enabled {
                        field = Some((
                            instance.enabled,
                            changed,
                            instance.kind,
                            instance.surrounding.clone(),
                            surrounding_changed,
                        ));
                    }
                }
                zwp_text_input_v3::Request::SetContentType { purpose, .. } if focused => {
                    instance.pending_kind = Some(field_kind(purpose));
                }
                zwp_text_input_v3::Request::SetSurroundingText { text, cursor, anchor } if focused => {
                    instance.set_surrounding(text, cursor, anchor);
                }
                zwp_text_input_v3::Request::Destroy
                | zwp_text_input_v3::Request::Enable
                | zwp_text_input_v3::Request::SetSurroundingText { .. }
                | zwp_text_input_v3::Request::SetTextChangeCause { .. }
                | zwp_text_input_v3::Request::SetContentType { .. }
                | zwp_text_input_v3::Request::SetCursorRectangle { .. }
                | zwp_text_input_v3::Request::SetAvailableActions { .. }
                | zwp_text_input_v3::Request::ShowInputPanel
                | zwp_text_input_v3::Request::HideInputPanel => {}
                _ => tracing::warn!("unsupported text-input request"),
            }
        }
        if let Some((active, changed, kind, surrounding, surrounding_changed)) = field {
            let _rejection = state.dispatch(tehuti_core::dispatch::Input::TextField {
                active,
                changed,
                kind,
            });
            if surrounding_changed || !active {
                state.set_keyboard_surrounding(if active { surrounding } else { None });
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        resource: &ZwpTextInputV3,
        (): &(),
    ) {
        let was_enabled = state
            .text_input
            .instances
            .iter()
            .any(|instance| instance.resource == *resource && instance.enabled);
        state
            .text_input
            .instances
            .retain(|instance| instance.resource != *resource);
        if was_enabled {
            let _rejection = state.dispatch(tehuti_core::dispatch::Input::TextField {
                active: false,
                changed: false,
                kind: FieldKind::Normal,
            });
        }
    }
}

const fn field_kind(
    purpose: smithay::reexports::wayland_server::WEnum<zwp_text_input_v3::ContentPurpose>,
) -> FieldKind {
    use smithay::reexports::wayland_server::WEnum;
    use zwp_text_input_v3::ContentPurpose;
    match purpose {
        WEnum::Value(ContentPurpose::Url) => FieldKind::Url,
        WEnum::Value(ContentPurpose::Email) => FieldKind::Email,
        WEnum::Value(ContentPurpose::Password) => FieldKind::Password,
        WEnum::Value(ContentPurpose::Terminal) => FieldKind::Terminal,
        WEnum::Value(_) | WEnum::Unknown(_) => FieldKind::Normal,
    }
}

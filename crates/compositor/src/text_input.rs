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

#[derive(Debug)]
struct Instance {
    resource: ZwpTextInputV3,
    pending_enabled: Option<bool>,
    enabled: bool,
    serial: u32,
    terminal: bool,
    pending_terminal: Option<bool>,
    kind: FieldKind,
    pending_kind: Option<FieldKind>,
}

impl Instance {
    fn commit(&mut self, focused: bool) -> bool {
        self.serial = self.serial.wrapping_add(1);
        let changed = self.pending_enabled == Some(true);
        if let Some(kind) = self.pending_kind.take() {
            self.kind = kind;
        }
        if let Some(terminal) = self.pending_terminal.take() {
            self.terminal = terminal;
        }
        if let Some(enabled) = self.pending_enabled.take() {
            self.enabled = enabled && focused;
        }
        changed
    }
}

impl TextInput {
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
            instance.terminal = false;
            instance.pending_terminal = None;
            instance.kind = FieldKind::Normal;
            instance.pending_kind = None;
            if let Some(surface) = &focus
                && instance.resource.id().same_client_as(&surface.id())
            {
                instance.resource.enter(surface);
            }
        }
        self.focus = focus;
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
                terminal: false,
                pending_terminal: None,
                kind: FieldKind::Normal,
                pending_kind: None,
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
        let focused = state
            .text_input
            .focus
            .as_ref()
            .is_some_and(|surface| resource.id().same_client_as(&surface.id()));
        let another_enabled = state
            .text_input
            .instances
            .iter()
            .any(|instance| instance.resource != *resource && instance.enabled);
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
                    instance.pending_terminal = Some(false);
                    instance.pending_kind = Some(FieldKind::Normal);
                }
                zwp_text_input_v3::Request::Disable => instance.pending_enabled = Some(false),
                zwp_text_input_v3::Request::Commit => {
                    let changed = instance.commit(focused);
                    if focused && !another_enabled {
                        field = Some((instance.enabled && !instance.terminal, changed, instance.kind));
                    }
                }
                zwp_text_input_v3::Request::SetContentType { purpose, .. } if focused => {
                    instance.pending_kind = Some(field_kind(purpose));
                    instance.pending_terminal = Some(
                        purpose
                            == smithay::reexports::wayland_server::WEnum::Value(
                                zwp_text_input_v3::ContentPurpose::Terminal,
                            ),
                    );
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
        if let Some((active, changed, kind)) = field {
            let _rejection = state.dispatch(tehuti_core::dispatch::Input::TextField {
                active,
                changed,
                kind,
            });
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
        WEnum::Value(_) | WEnum::Unknown(_) => FieldKind::Normal,
    }
}

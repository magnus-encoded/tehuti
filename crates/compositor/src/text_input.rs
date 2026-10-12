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
            if let Some(surface) = &focus
                && instance.resource.id().same_client_as(&surface.id())
            {
                instance.resource.enter(surface);
            }
        }
        self.focus = focus;
    }

    pub(crate) fn send(&self, text: &str) {
        if text.contains('\0') {
            tracing::warn!("text action rejected: Wayland strings cannot carry NUL");
            return;
        }
        let Some(surface) = self.focus.as_ref().filter(|surface| surface.is_alive()) else {
            return;
        };
        for instance in &self.instances {
            if instance.enabled && instance.resource.id().same_client_as(&surface.id()) {
                // https://wayland.app/protocols/text-input-unstable-v3: strings fit in 4000 bytes.
                let mut remaining = text;
                while !remaining.is_empty() {
                    let end = remaining.floor_char_boundary(remaining.len().min(4000));
                    let Some((chunk, rest)) = remaining.split_at_checked(end) else {
                        return;
                    };
                    instance.resource.commit_string(Some(chunk.into()));
                    instance.resource.done(instance.serial);
                    remaining = rest;
                }
            }
        }
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
        if let Some(instance) = state
            .text_input
            .instances
            .iter_mut()
            .find(|instance| instance.resource == *resource)
        {
            match request {
                zwp_text_input_v3::Request::Enable if focused => instance.pending_enabled = Some(true),
                zwp_text_input_v3::Request::Disable => instance.pending_enabled = Some(false),
                zwp_text_input_v3::Request::Commit => {
                    instance.serial = instance.serial.wrapping_add(1);
                    if let Some(enabled) = instance.pending_enabled.take() {
                        instance.enabled = enabled && focused;
                    }
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
    }

    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        resource: &ZwpTextInputV3,
        (): &(),
    ) {
        state
            .text_input
            .instances
            .retain(|instance| instance.resource != *resource);
    }
}

//! Tehuti's compositor edge: Smithay state, input sources, and the effects of core actions.
#![expect(
    clippy::multiple_crate_versions,
    reason = "Smithay and winit pull in duplicate transitive versions outside our control"
)]

pub mod controller;
pub mod input;
pub mod state;

pub mod contexts;

pub(crate) mod text_input;

pub mod plugins;

pub mod speech;

pub mod notice;

pub mod secret_delivery;

pub mod desktop;

pub mod backend;

pub mod dim;
mod focus_outline;
mod layout_window;

pub mod shutdown;

pub mod rendering;

pub mod scale;

pub mod bar;

pub mod osk;
pub mod wallpaper;

pub mod display_mode_store;
pub mod input_choice_store;
pub mod settings;
pub mod system_menu;

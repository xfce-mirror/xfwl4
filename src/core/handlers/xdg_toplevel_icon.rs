use smithay::{delegate_xdg_toplevel_icon, wayland::xdg_toplevel_icon::XdgToplevelIconHandler};

use crate::{backend::Backend, core::state::Xfwl4State};

impl<BackendData: Backend + 'static> XdgToplevelIconHandler for Xfwl4State<BackendData> {}

delegate_xdg_toplevel_icon!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

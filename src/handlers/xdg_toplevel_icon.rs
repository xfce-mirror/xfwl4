use smithay::{delegate_xdg_toplevel_icon, wayland::xdg_toplevel_icon::XdgToplevelIconHandler};

use crate::{Xfwl4State, backend::Backend};

impl<BackendData: Backend + 'static> XdgToplevelIconHandler for Xfwl4State<BackendData> {}

delegate_xdg_toplevel_icon!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

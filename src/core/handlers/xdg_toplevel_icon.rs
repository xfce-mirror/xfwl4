use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconHandler;

use crate::{backend::Backend, core::state::Xfwl4State};

impl<BackendData: Backend + 'static> XdgToplevelIconHandler for Xfwl4State<BackendData> {}

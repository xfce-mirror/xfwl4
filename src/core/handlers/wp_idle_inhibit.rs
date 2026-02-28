use smithay::{
    delegate_idle_inhibit, reexports::wayland_server::protocol::wl_surface::WlSurface, wayland::idle_inhibit::IdleInhibitHandler,
};

use crate::{backend::Backend, core::state::Xfwl4State};

impl<BackendData: Backend + 'static> IdleInhibitHandler for Xfwl4State<BackendData> {
    fn inhibit(&mut self, surface: WlSurface) {
        let was_empty = self.idle_inhibit_surfaces.is_empty();
        self.idle_inhibit_surfaces.insert(surface);

        if was_empty {
            self.ext_idle_notifier_state.set_is_inhibited(true);
        }
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        let was_empty = self.idle_inhibit_surfaces.is_empty();
        self.idle_inhibit_surfaces.remove(&surface);

        if !was_empty && self.idle_inhibit_surfaces.is_empty() {
            self.ext_idle_notifier_state.set_is_inhibited(false);
        }
    }
}

delegate_idle_inhibit!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

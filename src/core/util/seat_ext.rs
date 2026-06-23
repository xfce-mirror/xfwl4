// xfwl4 -- Wayland compositor for the Xfce Desktop Environment
//
// Copyright (C) 2026 Brian Tarricone <brian@tarricone.org>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use smithay::{
    input::{Seat, SeatHandler},
    reexports::wayland_server::{Client, Resource},
    utils::IsAlive,
    wayland::seat::WaylandFocus,
};

use crate::core::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    shell::SSD,
};

pub trait SeatFocusExt {
    fn keyboard_client(&self) -> Option<Client>;
    fn pointer_client(&self) -> Option<Client>;
}

impl<D> SeatFocusExt for Seat<D>
where
    D: SeatHandler<KeyboardFocus = KeyboardFocusTarget, PointerFocus = PointerFocusTarget> + 'static,
{
    fn keyboard_client(&self) -> Option<Client> {
        self.get_keyboard().and_then(|keyboard| {
            keyboard.current_focus().and_then(|focus| {
                focus
                    .alive()
                    .then(|| match focus {
                        KeyboardFocusTarget::Window(window) => window.wl_surface().and_then(|wl_surface| wl_surface.client()),
                        KeyboardFocusTarget::Popup(popup) => popup.wl_surface().client(),
                        KeyboardFocusTarget::LayerSurface(surface) => surface.wl_surface().client(),
                    })
                    .flatten()
            })
        })
    }

    fn pointer_client(&self) -> Option<Client> {
        self.get_pointer().and_then(|pointer| {
            pointer.current_focus().and_then(|focus| {
                focus
                    .alive()
                    .then(|| match focus {
                        PointerFocusTarget::WlSurface(wl_surface) => wl_surface.client(),
                        #[cfg(feature = "xwayland")]
                        PointerFocusTarget::X11Surface(x11_surface) => x11_surface.wl_surface().and_then(|wl_surface| wl_surface.client()),
                        PointerFocusTarget::SSD(SSD(window)) => window.0.wl_surface().and_then(|wl_surface| wl_surface.client()),
                    })
                    .flatten()
            })
        })
    }
}

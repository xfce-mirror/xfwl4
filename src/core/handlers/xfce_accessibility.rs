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

use std::{collections::HashMap, time::Duration};

use calloop::timer::{TimeoutAction, Timer};
use smithay::{
    input::{Seat, pointer::PointerHandle},
    reexports::wayland_server::DisplayHandle,
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{
        drawing::find_pointer::FindPointerOverlay,
        state::{Xfwl4Core, Xfwl4State},
    },
    protocols::xfce_accessibility::{XfceAccessibilityHandler, XfceAccessibilityState},
};

const FIND_POINTER_FRAME_TIMEOUT: Duration = Duration::from_millis(10);

pub struct AccessibilityState<BackendData: Backend + 'static> {
    xfce_accessibility_state: XfceAccessibilityState<Xfwl4State<BackendData>>,
    channel: xfconf::Channel,
    find_pointer_overlays: HashMap<PointerHandle<Xfwl4State<BackendData>>, FindPointerOverlay>,
}

impl<BackendData: Backend + 'static> AccessibilityState<BackendData> {
    pub fn new(dh: &DisplayHandle) -> Self {
        let xfce_accessibility_state = XfceAccessibilityState::new(dh);
        let channel = xfconf::Channel::new("accessibility");
        Self {
            xfce_accessibility_state,
            channel,
            find_pointer_overlays: HashMap::new(),
        }
    }
}

impl<BackendData: Backend + 'static> XfceAccessibilityHandler for Xfwl4State<BackendData> {
    fn xfce_accessibility_state(&mut self) -> &mut crate::protocols::xfce_accessibility::XfceAccessibilityState<Self> {
        &mut self.core.protocol_delegates.accessibility_state.xfce_accessibility_state
    }

    fn find_pointer(&mut self, seat: Seat<Self>) {
        if self
            .core
            .protocol_delegates
            .accessibility_state
            .channel
            .get_property::<bool>("/FindCursor")
            .unwrap_or(false)
            && let Some(pointer) = seat.get_pointer()
        {
            self.core.register_timer(Timer::from_duration(FIND_POINTER_FRAME_TIMEOUT), {
                let pointer = pointer.clone();
                move |state| {
                    let action = if let Some(overlay) = state
                        .core
                        .protocol_delegates
                        .accessibility_state
                        .find_pointer_overlays
                        .get(&pointer)
                        && !overlay.finished()
                    {
                        TimeoutAction::ToDuration(FIND_POINTER_FRAME_TIMEOUT)
                    } else {
                        state
                            .core
                            .protocol_delegates
                            .accessibility_state
                            .find_pointer_overlays
                            .remove(&pointer);
                        state
                            .core
                            .protocol_delegates
                            .accessibility_state
                            .xfce_accessibility_state
                            .find_pointer_completed(&seat);
                        TimeoutAction::Drop
                    };

                    state.schedule_render();
                    action
                }
            });

            self.core
                .protocol_delegates
                .accessibility_state
                .find_pointer_overlays
                .insert(pointer, FindPointerOverlay::new());
        } else {
            self.core
                .protocol_delegates
                .accessibility_state
                .xfce_accessibility_state
                .find_pointer_completed(&seat);
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    pub(in crate::core) fn overlay_for_pointer_mut(
        &mut self,
        pointer: &PointerHandle<Xfwl4State<BackendData>>,
    ) -> Option<&mut FindPointerOverlay> {
        self.protocol_delegates
            .accessibility_state
            .find_pointer_overlays
            .iter_mut()
            .find_map(|(a_pointer, overlay)| (a_pointer == pointer).then_some(overlay))
    }
}

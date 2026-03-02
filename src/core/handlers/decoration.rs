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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::collections::HashMap;

use smithay::{
    delegate_kde_decoration, delegate_xdg_decoration,
    reexports::{
        calloop::LoopHandle,
        wayland_protocols::xdg::decoration::{self as xdg_decoration, zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode},
        wayland_protocols_misc::server_decoration::server::{
            org_kde_kwin_server_decoration::{Mode as KdeDecorationMode, OrgKdeKwinServerDecoration},
            org_kde_kwin_server_decoration_manager::Mode as KdeDefaultDecorationMode,
        },
        wayland_server::{DisplayHandle, Resource, WEnum, backend::ObjectId, protocol::wl_surface::WlSurface},
    },
    wayland::{
        compositor,
        shell::{
            kde::decoration::{KdeDecorationHandler, KdeDecorationState},
            xdg::{
                ToplevelSurface,
                decoration::{XdgDecorationHandler, XdgDecorationState},
            },
        },
    },
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";
const PROP_DIALOGS_USE_HEADER: &str = "/Gtk/DialogsUseHeader";

struct KdeDecoration {
    _decoration: OrgKdeKwinServerDecoration,
    last_request: Option<WEnum<KdeDecorationMode>>,
}

pub struct DecorationState {
    channel: xfconf::Channel,
    default_mode: XdgDecorationMode,

    _xdg_decoration_state: XdgDecorationState,
    toplevels: HashMap<ObjectId, ToplevelSurface>,

    kde_decoration_state: KdeDecorationState,
    kde_decorations: HashMap<ObjectId, KdeDecoration>,
}

impl DecorationState {
    pub fn new<BackendData: Backend + 'static>(dh: &DisplayHandle, lh: LoopHandle<'static, Xfwl4State<BackendData>>) -> Self {
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);
        let dialogs_use_header = channel.get_property::<bool>(PROP_DIALOGS_USE_HEADER).unwrap_or(false);
        let default_mode = if dialogs_use_header {
            XdgDecorationMode::ClientSide
        } else {
            XdgDecorationMode::ServerSide
        };

        let state = Self {
            channel: xfconf::Channel::new(XSETTINGS_CHANNEL_NAME),
            default_mode,
            _xdg_decoration_state: XdgDecorationState::new::<Xfwl4State<BackendData>>(dh),
            toplevels: HashMap::new(),
            kde_decoration_state: KdeDecorationState::new::<Xfwl4State<BackendData>>(dh, xdg_mode_to_kde_default_mode(default_mode)),
            kde_decorations: HashMap::new(),
        };

        let source = CalloopXfconfSource::new(state.channel.clone(), [PROP_DIALOGS_USE_HEADER]);
        lh.insert_source(source, |(_, value), _, state| {
            if let Ok(dialogs_use_header) = value.get::<bool>() {
                let new_default_mode = if dialogs_use_header {
                    XdgDecorationMode::ClientSide
                } else {
                    XdgDecorationMode::ServerSide
                };

                let state = &mut state.core.protocol_delegates.decoration_state;

                if state.default_mode != new_default_mode {
                    state.default_mode = new_default_mode;

                    state
                        .kde_decoration_state
                        .set_default_mode(xdg_mode_to_kde_default_mode(new_default_mode));

                    // Ideally we could tell clients about the new mode the user wants, but in
                    // practice, some apps (notably GTK3, at least) will not actually switch their
                    // own decoration state when we advertise a new default mode or even try to
                    // send new state to individual decoration objects.  So we end up with a bad
                    // state: if we switch from CSD to SSD, then windows end up with *both* CSDs
                    // and SSDs, and if we switch from SSD to CSD, then windows end up with no
                    // decorations at all.
                    /*
                    for toplevel in state.toplevels.values() {
                        toplevel.with_pending_state(|state| {
                            state.decoration_mode = Some(new_default_mode);
                        });

                        if toplevel.is_initial_configure_sent() {
                            toplevel.send_pending_configure();
                        }
                    }

                    let kde_mode = xdg_mode_to_kde_mode(new_default_mode);
                    for kde_decoration in state.kde_decorations.values() {
                        kde_decoration.decoration.mode(kde_mode);
                    }
                    */
                }
            }
        })
        .unwrap();

        state
    }
}

impl<BackendData: Backend> XdgDecorationHandler for Xfwl4State<BackendData> {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(self.core.protocol_delegates.decoration_state.default_mode);
        });

        let surface = toplevel.wl_surface();
        self.core
            .protocol_delegates
            .decoration_state
            .toplevels
            .insert(surface.id(), toplevel.clone());

        compositor::add_destruction_hook::<Self, _>(surface, |state, surface| {
            state.core.protocol_delegates.decoration_state.toplevels.remove(&surface.id());
        });
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: XdgDecorationMode) {
        use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;

        // Whether or not we honor the client's request depends on a few things:
        // 1. If the client requests server-side, we always respect it, because the canonical case
        //    for that is the client cannot draw its own decorations.
        // 2. If the client requests client-side, but the user configuration is server-side, we
        //    reject the request and tell the client server-side.
        // 3. If the client requestsclient-side, and the user configuration is also client-side, we
        //    honor the request and tell the client client-side.

        let final_mode = match (self.core.protocol_delegates.decoration_state.default_mode, mode) {
            (_, Mode::ServerSide) => Mode::ServerSide,
            (Mode::ServerSide, Mode::ClientSide) => Mode::ServerSide,
            (Mode::ClientSide, Mode::ClientSide) => Mode::ClientSide,
            _ => Mode::ServerSide,
        };

        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(final_mode);
        });

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(self.core.protocol_delegates.decoration_state.default_mode);
        });

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

delegate_xdg_decoration!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> KdeDecorationHandler for Xfwl4State<BackendData> {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.core.protocol_delegates.decoration_state.kde_decoration_state
    }

    fn new_decoration(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration) {
        self.core.protocol_delegates.decoration_state.kde_decorations.insert(
            surface.id(),
            KdeDecoration {
                _decoration: decoration.clone(),
                last_request: None,
            },
        );
        decoration.mode(xdg_mode_to_kde_mode(self.core.protocol_delegates.decoration_state.default_mode));

        self.update_decoration_state_for_kde(surface, self.core.protocol_delegates.decoration_state.default_mode);
    }

    fn request_mode(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration, mode: WEnum<KdeDecorationMode>) {
        if let Some(kde_decoration) = self.core.protocol_delegates.decoration_state.kde_decorations.get_mut(&surface.id()) {
            let final_mode = if kde_decoration.last_request.as_ref().is_some_and(|lr| *lr == mode) {
                // Might have a loop, so acquiesce to whatever they want
                if let WEnum::Value(mode) = mode { Some(mode) } else { None }
            } else {
                kde_decoration.last_request = Some(mode);

                Some(if let WEnum::Value(mode) = mode {
                    // See XdgDecorationHandler::request_mode() above for rationale.
                    match (self.core.protocol_delegates.decoration_state.default_mode, mode) {
                        (_, KdeDecorationMode::Server) => KdeDecorationMode::Server,
                        (_, KdeDecorationMode::None) => KdeDecorationMode::None,
                        (XdgDecorationMode::ServerSide, KdeDecorationMode::Client) => KdeDecorationMode::Server,
                        (XdgDecorationMode::ClientSide, KdeDecorationMode::Client) => KdeDecorationMode::Client,
                        _ => KdeDecorationMode::Server,
                    }
                } else {
                    xdg_mode_to_kde_mode(self.core.protocol_delegates.decoration_state.default_mode)
                })
            };

            if let Some(final_mode) = final_mode {
                decoration.mode(final_mode);
                self.update_decoration_state_for_kde(surface, kde_mode_to_xdg_mode(final_mode));
            }
        }
    }

    fn release(&mut self, _decoration: &OrgKdeKwinServerDecoration, surface: &WlSurface) {
        self.core.protocol_delegates.decoration_state.kde_decorations.remove(&surface.id());
    }
}

delegate_kde_decoration!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn update_decoration_state_for_kde(&mut self, surface: &WlSurface, mode: XdgDecorationMode) {
        if let Some(window) = self.window_for_surface(surface) {
            if let Some(toplevel) = window.0.toplevel() {
                let mode = if !self.window_is_tabwin(&window, surface) {
                    mode
                } else {
                    XdgDecorationMode::ClientSide
                };

                // XdgShellHandler::ack_configure() will update SSD/CSD status for the window, so
                // fake the xdg_decoration state and send a configure.  We can't just call
                // set_ssd() on the WindowElement directly here, because a later ack_configure()
                // event could override this, as it knows nothing about the KDE decoration state.

                let changed = toplevel.with_pending_state(|state| {
                    if state.decoration_mode.is_none_or(|cur_mode| cur_mode != mode) {
                        state.decoration_mode = Some(mode);
                        true
                    } else {
                        false
                    }
                });

                if changed && toplevel.is_initial_configure_sent() {
                    toplevel.send_configure();
                }
            } else if mode == XdgDecorationMode::ServerSide {
                self.enable_decorations_for_window(&window);
            } else {
                window.disable_decorations();
            }
        }
    }
}

fn xdg_mode_to_kde_default_mode(mode: XdgDecorationMode) -> KdeDefaultDecorationMode {
    match mode {
        XdgDecorationMode::ClientSide => KdeDefaultDecorationMode::Client,
        XdgDecorationMode::ServerSide => KdeDefaultDecorationMode::Server,
        _ => unreachable!(),
    }
}

fn xdg_mode_to_kde_mode(mode: XdgDecorationMode) -> KdeDecorationMode {
    match mode {
        XdgDecorationMode::ClientSide => KdeDecorationMode::Client,
        XdgDecorationMode::ServerSide => KdeDecorationMode::Server,
        _ => unreachable!(),
    }
}

fn kde_mode_to_xdg_mode(mode: KdeDecorationMode) -> XdgDecorationMode {
    match mode {
        KdeDecorationMode::None | KdeDecorationMode::Client => XdgDecorationMode::ClientSide,
        KdeDecorationMode::Server => XdgDecorationMode::ServerSide,
        _ => unreachable!(),
    }
}

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

use crate::{Xfwl4State, backend::Backend};

pub mod data_device;
mod decoration;
mod ext_idle_notify;
mod fractional_scale;
mod input;
mod output;
mod seat;
mod security_context;
mod shm;
mod wlr_gamma_control;
mod wp_idle_inhibit;
mod xdg_activation;
mod xdg_foreign;
mod xdg_toplevel_icon;

pub(crate) use decoration::DecorationState;

smithay::delegate_viewporter!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_presentation!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_single_pixel_buffer!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_fifo!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_commit_timing!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_fixes!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

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

use smithay::{
    backend::renderer::{
        Color32F, ImportAll, ImportMem, Renderer, RendererSuper,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{AsRenderElements, Element, Kind, RenderElement, Wrap, surface::WaylandSurfaceRenderElement},
    },
    desktop::space::SpaceRenderElements,
    output::Output,
    render_elements,
    wayland::compositor,
};

use crate::{
    backend::{AsGlesRenderer, FromGlesError},
    drawing::{CLEAR_COLOR, CLEAR_COLOR_FULLSCREEN, PointerRenderElement},
    handlers::ExtSessionLockState,
    shell::{WindowElement, WindowRenderElement},
    workspaces::Workspace,
};

render_elements! {
    pub CustomRenderElements<R> where
        R: ImportAll + ImportMem;
    Pointer=PointerRenderElement<R>,
    Surface=WaylandSurfaceRenderElement<R>,
    #[cfg(feature = "debug")]
    // Note: We would like to borrow this element instead, but that would introduce
    // a feature-dependent lifetime, which introduces a lot more feature bounds
    // as the whole type changes and we can't have an unused lifetime (for when "debug" is disabled)
    // in the declaration.
    Fps=crate::debug::FpsElement<R::TextureId>,
}

impl<R: Renderer> std::fmt::Debug for CustomRenderElements<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pointer(arg0) => f.debug_tuple("Pointer").field(arg0).finish(),
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            #[cfg(feature = "debug")]
            Self::Fps(arg0) => f.debug_tuple("Fps").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

render_elements! {
    pub OutputRenderElements<R, E> where
        R: ImportAll + ImportMem + AsGlesRenderer,
        <R as RendererSuper>::Error: FromGlesError;
    Space=SpaceRenderElements<R, E>,
    Window=Wrap<E>,
    Custom=CustomRenderElements<R>,
}

impl<R, E> std::fmt::Debug for OutputRenderElements<R, E>
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    <R as RendererSuper>::Error: FromGlesError,
    E: RenderElement<R> + Element,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputRenderElements::Space(_) => f.debug_tuple("Space").finish_non_exhaustive(),
            OutputRenderElements::Window(_) => f.debug_tuple("Window").finish_non_exhaustive(),
            OutputRenderElements::Custom(_) => f.debug_tuple("Custom").finish_non_exhaustive(),
            OutputRenderElements::_GenericCatcher(_) => unreachable!(),
        }
    }
}

pub struct RenderView<'a, R> {
    pub ext_session_lock_state: &'a ExtSessionLockState,
    pub renderer: &'a mut R,
}

impl<'a, R> RenderView<'a, R>
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    R::TextureId: Clone + 'static,
    <R as smithay::backend::renderer::RendererSuper>::Error: FromGlesError,
{
    #[profiling::function]
    pub fn output_elements(
        &mut self,
        output: &Output,
        workspace: &Workspace,
        custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
    ) -> (Vec<OutputRenderElements<R, WindowRenderElement<R>>>, Color32F) {
        if let Some(lock_surface) = self.ext_session_lock_state.lock_surface_for_output(output) {
            match compositor::with_states(lock_surface.wl_surface(), |states| {
                WaylandSurfaceRenderElement::from_surface(
                    self.renderer,
                    lock_surface.wl_surface(),
                    states,
                    output
                        .current_location()
                        .to_f64()
                        .to_physical(output.current_scale().fractional_scale()),
                    1.,
                    Kind::Unspecified,
                )
            }) {
                Ok(Some(elem)) => (
                    vec![OutputRenderElements::Custom(CustomRenderElements::Surface(elem))],
                    CLEAR_COLOR_FULLSCREEN,
                ),
                Ok(None) => {
                    tracing::warn!("Failed to create render element from lockscreen surface");
                    (vec![], CLEAR_COLOR_FULLSCREEN)
                }
                Err(err) => {
                    tracing::warn!("Failed to create render element from lockscreen surface: {err}");
                    (vec![], CLEAR_COLOR_FULLSCREEN)
                }
            }
        } else if let Some(window) = workspace.fullscreen_window_for_output(output) {
            let scale = output.current_scale().fractional_scale().into();
            let window_render_elements: Vec<WindowRenderElement<R>> =
                AsRenderElements::<R>::render_elements(&window, self.renderer, (0, 0).into(), scale, 1.0);

            let elements = custom_elements
                .into_iter()
                .map(OutputRenderElements::from)
                .chain(
                    window_render_elements
                        .into_iter()
                        .map(|e| OutputRenderElements::Window(Wrap::from(e))),
                )
                .collect::<Vec<_>>();
            (elements, CLEAR_COLOR_FULLSCREEN)
        } else {
            let mut output_render_elements = custom_elements.into_iter().map(OutputRenderElements::from).collect::<Vec<_>>();

            let space_elements =
                smithay::desktop::space::space_render_elements::<_, WindowElement, _>(self.renderer, [workspace.space()], output, 1.0)
                    .expect("output without mode?");
            output_render_elements.extend(space_elements.into_iter().map(OutputRenderElements::Space));

            (output_render_elements, CLEAR_COLOR)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_output<'d>(
        &mut self,
        output: &'a Output,
        workspace: &'a Workspace,
        custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
        framebuffer: &'a mut R::Framebuffer<'_>,
        damage_tracker: &'d mut OutputDamageTracker,
        age: usize,
    ) -> Result<RenderOutputResult<'d>, OutputDamageTrackerError<R::Error>> {
        let (elements, clear_color) = self.output_elements(output, workspace, custom_elements);
        damage_tracker.render_output(self.renderer, framebuffer, age, &elements, clear_color)
    }
}

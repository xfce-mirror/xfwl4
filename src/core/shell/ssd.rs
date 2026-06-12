// xfwl4 -- Wayland compositor for the Xfce Desktop Environmenth
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

use anyhow::anyhow;
use gtk::{cairo, pango};
use smithay::{
    backend::renderer::{
        ContextId, Renderer, Texture,
        element::{AsRenderElements, Id, Kind, texture::TextureRenderElement},
        gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformValue, element::TextureShaderElement},
    },
    desktop::{WindowSurface, space::SpaceElement},
    input::{Seat, pointer::CursorIcon},
    output::Scale as OutputScale,
    reexports::pixman,
    render_elements,
    utils::{Buffer, FrameExtents, Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform},
};

use std::{
    cell::{Ref, RefCell, RefMut},
    collections::{HashMap, HashSet},
    time::Duration,
};

use crate::{
    backend::Backend,
    core::{
        config::{DoubleClickAction, TitleAlignment, TitlebarButton, Xfwl4Config},
        drawing::{
            decorations::{
                DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode, DecorTexture,
                DecorTitleTextures, DecorationTheme, Direction,
            },
            shadows::{ShadowKey, ShadowParams},
            ssd::{DecorationRenderState, PixelBuffer, create_title_layout, icon_extents_for, render_title_text_pixels},
        },
        handlers::xfwl4_compositor_ui::ActionLocation,
        placement::FillMode,
        shell::{
            GrabTrigger, ResizeEdge,
            xdg::{desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel, window_title_for_xdg_toplevel},
        },
        state::Xfwl4State,
        util::{BTN_LEFT, BTN_RIGHT, BufferSizeExt, ImageData, ScrollAccumulator, icon_theme::FreedesktopIconsIconTheme},
    },
};

use super::WindowElement;

pub struct WindowState {
    window_decorations: Option<WindowDecorations>,
}

impl WindowState {
    pub fn has_decorations(&self) -> bool {
        self.window_decorations.is_some()
    }

    pub fn window_decorations(&self) -> Option<&WindowDecorations> {
        self.window_decorations.as_ref()
    }

    pub fn window_decorations_mut(&mut self) -> Option<&mut WindowDecorations> {
        self.window_decorations.as_mut()
    }
}

fn point_in_rect(rect: &Rectangle<i32, Physical>, loc: Point<f64, Physical>) -> bool {
    !rect.is_empty() && rect.contains(loc.to_i32_ceil())
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub(in crate::core) enum TitleLayout {
    TitleStretched {
        extents: Rectangle<i32, Physical>,
    },
    Title5Part {
        title1: Rectangle<i32, Physical>,
        top1: Rectangle<i32, Physical>,
        title2: Rectangle<i32, Physical>,
        top2: Rectangle<i32, Physical>,
        title3: Rectangle<i32, Physical>,
        top3: Rectangle<i32, Physical>,
        title4: Rectangle<i32, Physical>,
        top4: Rectangle<i32, Physical>,
        title5: Rectangle<i32, Physical>,
        top5: Rectangle<i32, Physical>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum HoverState {
    None,
    Close,
    Hide,
    Maximize,
    Menu,
    Shade,
    Stick,
    Titlebar,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum PressedState {
    None,
    Close,
    Hide,
    Maximize,
    Menu,
    Shade,
    Stick,
    Titlebar,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub(in crate::core) struct ButtonToggledStates: u8 {
        const Maximize = (1 << 0);
        const Shade = (1 << 1);
        const Stick = (1 << 2);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum TitlebarBlinkState {
    #[default]
    None,
    Active,
    Inactive,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct DirtyFlags: u8 {
        const TITLEBAR = 1;
        const TITLE_TEXT = 2;
    }
}

#[derive(Debug, Clone)]
pub(in crate::core) struct DecorationLayout {
    pub top_left: Rectangle<i32, Physical>,
    pub top: Rectangle<i32, Physical>,
    pub top_right: Rectangle<i32, Physical>,
    pub bottom_left: Rectangle<i32, Physical>,
    pub bottom: Rectangle<i32, Physical>,
    pub bottom_right: Rectangle<i32, Physical>,
    pub left: Rectangle<i32, Physical>,
    pub right: Rectangle<i32, Physical>,
    pub close: Rectangle<i32, Physical>,
    pub hide: Rectangle<i32, Physical>,
    pub maximize: Rectangle<i32, Physical>,
    pub menu: Rectangle<i32, Physical>,
    pub shade: Rectangle<i32, Physical>,
    pub stick: Rectangle<i32, Physical>,
    pub title_text: Rectangle<i32, Physical>,
    pub title_text_max_width: i32,
    pub titlebar: Rectangle<i32, Physical>,
    pub title: TitleLayout,
    pub top_clip: i32,
    pub shadow_offset: Point<i32, Physical>,
    pub shadow_size: Size<i32, Physical>,
    pub shadow_frame_size: Size<i32, Physical>,
}

impl DecorationLayout {
    fn zeroed() -> Self {
        Self {
            top_left: Rectangle::zero(),
            top: Rectangle::zero(),
            top_right: Rectangle::zero(),
            bottom_left: Rectangle::zero(),
            bottom: Rectangle::zero(),
            bottom_right: Rectangle::zero(),
            left: Rectangle::zero(),
            right: Rectangle::zero(),
            close: Rectangle::zero(),
            hide: Rectangle::zero(),
            maximize: Rectangle::zero(),
            menu: Rectangle::zero(),
            shade: Rectangle::zero(),
            stick: Rectangle::zero(),
            title_text: Rectangle::zero(),
            title_text_max_width: 0,
            titlebar: Rectangle::zero(),
            title: TitleLayout::TitleStretched {
                extents: Rectangle::zero(),
            },
            top_clip: 0,
            shadow_offset: Point::default(),
            shadow_size: Size::default(),
            shadow_frame_size: Size::default(),
        }
    }
}

// Render-scale cache key.  Quantized at 120ths -- Wayland's fractional-scale unit -- so every real
// output scale round-trips exactly through `scale()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScaleKey(u32);

impl ScaleKey {
    fn new(scale: f64) -> Self {
        ScaleKey((scale * 120.0).round() as u32)
    }

    fn scale(self) -> f64 {
        self.0 as f64 / 120.0
    }
}

// A decoration layout + composited titlebar built for one render scale.  Decorations are native px
// on every output (only the content region scales per output), so a window straddling two outputs
// of different scale draws from a separate, crisp entry per output rather than one decor-scale
// layout resampled onto the other.  The titlebar texture is regenerated on appearance-only changes
// (hover/press) while the layout is kept.
struct ScaledRender {
    layout: DecorationLayout,
    input_region: Vec<Rectangle<i32, Physical>>,
    title_text_pixels: Option<PixelBuffer>,
    window_icon_extents: Rectangle<i32, Physical>,
    titlebar_texture: RefCell<Option<GlesTexture>>,
}

impl std::fmt::Debug for ScaledRender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScaledRender")
            .field("layout", &self.layout)
            .field("window_icon_extents", &self.window_icon_extents)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct DoubleClickState {
    last_location: Point<f64, Logical>,
    last_time_msec: u32,
}

#[derive(Debug)]
pub struct WindowDecorations {
    pub pointer_loc: Option<Point<f64, Logical>>,
    window_size: Size<i32, Logical>,
    window_title: Option<String>,
    scale: OutputScale,
    config: Xfwl4Config,
    decoration_theme: DecorationTheme,
    icon_theme: FreedesktopIconsIconTheme,
    font_map: pango::FontMap,
    font_options: cairo::FontOptions,

    is_active: bool,
    hover_state: HoverState,
    pressed_state: PressedState,
    button_toggled_states: ButtonToggledStates,
    scroll_accumulator: ScrollAccumulator,
    hide_titlebar_when_maximized: bool,
    titlebar_double_click_state: Option<DoubleClickState>,
    titlebar_blink_state: TitlebarBlinkState,

    window_icon: Option<ImageData>,

    title_text_size: Size<i32, Physical>,

    layout: DecorationLayout,
    input_region: Vec<Rectangle<i32, Physical>>,
    render_state: DecorationRenderState,
    scaled: RefCell<HashMap<ScaleKey, ScaledRender>>,
}

impl WindowDecorations {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window_size: Size<i32, Logical>,
        window_title: Option<String>,
        hide_titlebar_when_maximized: bool,
        window_icon: Option<ImageData>,
        scale: OutputScale,
        config: Xfwl4Config,
        decoration_theme: DecorationTheme,
        icon_theme: FreedesktopIconsIconTheme,
        font_map: pango::FontMap,
        font_options: cairo::FontOptions,
    ) -> Self {
        let mut decorations = Self {
            pointer_loc: None,
            window_size,
            window_title,
            scale,
            config,
            decoration_theme,
            icon_theme,
            font_map,
            font_options,
            is_active: false,
            hover_state: HoverState::None,
            pressed_state: PressedState::None,
            button_toggled_states: ButtonToggledStates::empty(),
            scroll_accumulator: ScrollAccumulator::default(),
            hide_titlebar_when_maximized,
            titlebar_double_click_state: None,
            titlebar_blink_state: TitlebarBlinkState::default(),
            window_icon,
            title_text_size: Size::default(),
            layout: DecorationLayout::zeroed(),
            input_region: Vec::new(),
            render_state: DecorationRenderState::new(),
            scaled: RefCell::new(HashMap::new()),
        };
        let flags = decorations.recalculate_layout();
        decorations.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
        decorations
    }

    fn point_in_region(location: Point<f64, Logical>, scale: f64, region: &[Rectangle<i32, Physical>]) -> bool {
        let location = location.to_physical(scale).to_i32_ceil();
        region.iter().any(|rect| rect.contains(location))
    }

    // True if `point` (frame-physical px) lands on an opaque pixel of `button`'s resting bitmap.
    // Buttons are drawn AsIs (native px == physical px), so the bitmap's own opaque region, offset
    // to the button's placement, is the clickable area -- a click in the transparent gap of a
    // floating/round button falls through to whatever it overlaps rather than hitting the button.
    fn point_on_button(&self, button: TitlebarButton, extents: Rectangle<i32, Physical>, point: Point<i32, Physical>) -> bool {
        !extents.is_empty() && extents.contains(point) && {
            let local = Point::<i32, Buffer>::from((point.x - extents.loc.x, point.y - extents.loc.y));
            let bg_state = self.bg_state();
            let btn_name = DecorButtonName::from((button, self.button_toggled_states));
            self.decoration_theme
                .button_texture(btn_name, resting_button_state(bg_state), bg_state)
                .map(|texture| texture.opaque_regions().iter().any(|opaque| opaque.contains(local)))
                .unwrap_or(false)
        }
    }

    // The titlebar button whose opaque region `point` (frame-physical px) lands on, if any.  Shared
    // by hover and press hit-testing, which each map the button to their own state enum.
    fn button_at(&self, layout: &DecorationLayout, point: Point<i32, Physical>) -> Option<TitlebarButton> {
        [
            (TitlebarButton::Close, layout.close),
            (TitlebarButton::Hide, layout.hide),
            (TitlebarButton::Maximize, layout.maximize),
            (TitlebarButton::Menu, layout.menu),
            (TitlebarButton::Shade, layout.shade),
            (TitlebarButton::Stick, layout.stick),
        ]
        .into_iter()
        .find_map(|(button, extents)| self.point_on_button(button, extents, point).then_some(button))
    }

    // Tested against the input region the pointer's output rendered (decorations are native px per
    // output); `scale` is that output's scale, supplied by the caller, which knows the output.  The
    // region is the union of every piece's opaque pixels, so transparent gaps in the titlebar and
    // borders fall through rather than swallowing the click.
    pub fn point_is_in_decorations(&self, location: Point<f64, Logical>, scale: f64) -> bool {
        let scaled = self.scaled.borrow();
        scaled
            .get(&ScaleKey::new(scale))
            .map(|entry| Self::point_in_region(location, scale, &entry.input_region))
            .unwrap_or_else(|| Self::point_in_region(location, self.scale.fractional_scale(), &self.input_region))
    }

    // For `is_in_input_region`, which has no output context: a point is in the decorations if it
    // lands on them at any scale the window is currently rendered at (i.e. any output it spans).
    // The precise per-output decoration/content split is left to `surface_under`.
    pub fn point_is_in_any_decorations(&self, location: Point<f64, Logical>) -> bool {
        let scaled = self.scaled.borrow();
        if scaled.is_empty() {
            Self::point_in_region(location, self.scale.fractional_scale(), &self.input_region)
        } else {
            scaled
                .iter()
                .any(|(key, entry)| Self::point_in_region(location, key.scale(), &entry.input_region))
        }
    }

    pub fn decorations_extents_physical(&self) -> FrameExtents<i32, Physical> {
        FrameExtents::new(
            self.layout.left.size.w,
            self.layout.right.size.w,
            match &self.layout.title {
                TitleLayout::TitleStretched { extents } => extents.size.h,
                TitleLayout::Title5Part { title3, .. } => title3.size.h,
            },
            self.layout.bottom.size.h,
        )
    }

    pub fn decorations_extents(&self) -> FrameExtents<i32, Logical> {
        self.decorations_extents_physical()
            .to_f64()
            .to_logical(self.scale.fractional_scale())
            .to_i32_round()
    }

    pub fn decorations_offset(&self) -> Point<i32, Logical> {
        let extents = self.decorations_extents();
        (extents.left, extents.top).into()
    }

    pub fn decorations_offset_physical(&self) -> Point<i32, Physical> {
        let extents = self.decorations_extents_physical();
        (extents.left, extents.top).into()
    }

    fn shadow_extents_physical(&self) -> FrameExtents<i32, Physical> {
        if self.layout.shadow_size.is_empty() {
            FrameExtents::new(0, 0, 0, 0)
        } else {
            let left = (-self.layout.shadow_offset.x).max(0);
            let top = (-self.layout.shadow_offset.y).max(0);
            let right = (self.layout.shadow_offset.x + self.layout.shadow_size.w - self.layout.shadow_frame_size.w).max(0);
            let bottom = (self.layout.shadow_offset.y + self.layout.shadow_size.h - self.layout.shadow_frame_size.h).max(0);
            FrameExtents::new(left, right, top, bottom)
        }
    }

    pub fn shadow_extents(&self) -> FrameExtents<i32, Logical> {
        self.shadow_extents_physical()
            .to_f64()
            .to_logical(self.scale.fractional_scale())
            .to_i32_round()
    }

    // The smallest scale the window is currently rendered at, hence its largest logical decoration
    // footprint.  `bbox` uses it so the coarse element-under filter covers the (native px) decoration
    // and shadow as drawn on every output the window spans, not just its primary one.
    fn min_render_scale(&self) -> f64 {
        self.scaled
            .borrow()
            .keys()
            .map(|key| key.scale())
            .fold(self.scale.fractional_scale(), f64::min)
    }

    pub fn max_decorations_extents(&self) -> FrameExtents<i32, Logical> {
        self.decorations_extents_physical()
            .to_f64()
            .to_logical(self.min_render_scale())
            .to_i32_round()
    }

    pub fn max_shadow_extents(&self) -> FrameExtents<i32, Logical> {
        self.shadow_extents_physical()
            .to_f64()
            .to_logical(self.min_render_scale())
            .to_i32_round()
    }

    // Hit-testing uses the layout the pointer's output actually rendered, not the single
    // decoration-scale layout: decorations are native px on every output, so a window straddling
    // outputs of different scale has differently-sized decorations per output.  `pointer_loc` is
    // relative to the decoration origin, so adding the element's global origin tells us which
    // output the pointer is on (this also works for touch, where the seat pointer would be wrong).
    fn hit_test_layout<BackendData: Backend>(
        &self,
        state: &Xfwl4State<BackendData>,
        window: &WindowElement,
        pointer_loc: Point<f64, Logical>,
    ) -> (DecorationLayout, f64) {
        let origin = state
            .core
            .workspace_manager
            .window_location(window)
            .map(|loc| loc.to_f64())
            .unwrap_or_default();
        let scale = state
            .core
            .workspace_manager
            .output_scale_at(pointer_loc + origin)
            .fractional_scale();
        self.scaled
            .borrow()
            .get(&ScaleKey::new(scale))
            .map(|entry| (entry.layout.clone(), scale))
            .unwrap_or_else(|| (self.layout.clone(), self.scale.fractional_scale()))
    }

    pub fn pointer_motion<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        _serial: Serial,
        loc: Point<f64, Logical>,
    ) {
        self.pointer_loc = Some(loc);
        let (layout, scale) = self.hit_test_layout(state, window, loc);
        let loc_physical = loc.to_physical(scale);

        let new_hover_state = self
            .button_at(&layout, loc_physical.to_i32_ceil())
            .map_or(HoverState::None, HoverState::from);

        if new_hover_state != HoverState::None {
            if new_hover_state != self.hover_state {
                self.hover_state = new_hover_state;
                self.invalidate_render_state(DirtyFlags::TITLEBAR);
            }
            state.core.set_cursor(CursorIcon::Default);
        } else {
            let resize_grips = [
                (&layout.top_left, HoverState::TopLeft, CursorIcon::NwResize),
                (&layout.top, HoverState::Top, CursorIcon::NResize),
                (&layout.top_right, HoverState::TopRight, CursorIcon::NeResize),
                (&layout.left, HoverState::Left, CursorIcon::WResize),
                (&layout.right, HoverState::Right, CursorIcon::EResize),
                (&layout.bottom_left, HoverState::BottomLeft, CursorIcon::SwResize),
                (&layout.bottom, HoverState::Bottom, CursorIcon::SResize),
                (&layout.bottom_right, HoverState::BottomRight, CursorIcon::SeResize),
                (&layout.titlebar, HoverState::Titlebar, CursorIcon::Default),
            ];

            let (new_hover_state, new_cursor_name) = resize_grips
                .iter()
                .find_map(|(rect, flag, cursor)| point_in_rect(rect, loc_physical).then_some((*flag, *cursor)))
                .unwrap_or((HoverState::None, CursorIcon::Default));

            if new_hover_state != self.hover_state {
                state.core.set_cursor(new_cursor_name);
                self.hover_state = new_hover_state;
                self.invalidate_render_state(DirtyFlags::TITLEBAR);
            }
        }
    }

    pub fn pointer_leave<BackendData: Backend>(&mut self, state: &mut Xfwl4State<BackendData>) {
        self.pointer_loc = None;

        let needs_rerender = is_button_hover(self.hover_state) || is_button_pressed(self.pressed_state);

        match self.hover_state {
            HoverState::None => (),
            _ if !is_button_hover(self.hover_state) => state.core.set_cursor(CursorIcon::Default),
            _ => (),
        }
        self.hover_state = HoverState::None;
        self.pressed_state = PressedState::None;

        if needs_rerender {
            self.invalidate_render_state(DirtyFlags::TITLEBAR);
        }
    }

    fn button_press_or_touch_down<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        button: u32,
        serial: Serial,
        trigger: GrabTrigger,
    ) {
        if let Some(pointer_loc) = self.pointer_loc {
            let (layout, scale) = self.hit_test_layout(state, window, pointer_loc);
            let pointer_loc_physical = pointer_loc.to_physical(scale);

            let new_pressed_state = self
                .button_at(&layout, pointer_loc_physical.to_i32_ceil())
                .map_or(PressedState::None, PressedState::from);

            if new_pressed_state == PressedState::Menu {
                let window = window.clone();
                let seat = seat.clone();
                let location = pointer_loc.to_i32_round() - self.decorations_offset();
                state.core.handle.insert_idle(move |state| {
                    state.pop_up_window_menu(&window, &seat, serial, ActionLocation::WindowRelative(location));
                });
                // XXX: not bothering with a persistent pressed state for the menu button; I'm not
                // sure this is actually the right thing to do.
                self.pressed_state = PressedState::None;
            } else if new_pressed_state != PressedState::None {
                if new_pressed_state != self.pressed_state {
                    self.pressed_state = new_pressed_state;
                    self.invalidate_render_state(DirtyFlags::TITLEBAR);
                }
            } else {
                let titlebar_parts: Vec<(&Rectangle<i32, Physical>, PressedState)> = match &layout.title {
                    TitleLayout::TitleStretched { extents } => vec![(extents, PressedState::Titlebar)],
                    TitleLayout::Title5Part {
                        title1,
                        title2,
                        title3,
                        title4,
                        title5,
                        ..
                    } => {
                        vec![
                            (title1, PressedState::Titlebar),
                            (title2, PressedState::Titlebar),
                            (title3, PressedState::Titlebar),
                            (title4, PressedState::Titlebar),
                            (title5, PressedState::Titlebar),
                        ]
                    }
                };

                let resize_grips: [(&Rectangle<i32, Physical>, PressedState); 8] = [
                    (&layout.top_left, PressedState::TopLeft),
                    (&layout.top, PressedState::Top),
                    (&layout.top_right, PressedState::TopRight),
                    (&layout.left, PressedState::Left),
                    (&layout.right, PressedState::Right),
                    (&layout.bottom_left, PressedState::BottomLeft),
                    (&layout.bottom, PressedState::Bottom),
                    (&layout.bottom_right, PressedState::BottomRight),
                ];

                let mut move_resize_grips = resize_grips.into_iter().chain(titlebar_parts);

                let new_pressed_state = move_resize_grips
                    .find_map(|(rect, flag)| point_in_rect(rect, pointer_loc_physical).then_some(flag))
                    .unwrap_or(PressedState::None);

                if new_pressed_state != self.pressed_state {
                    self.pressed_state = new_pressed_state;

                    if new_pressed_state != PressedState::None {
                        let seat = seat.clone();
                        let window = window.clone();

                        if new_pressed_state == PressedState::Titlebar {
                            if button == BTN_LEFT {
                                state
                                    .core
                                    .handle
                                    .insert_idle(move |state| state.start_maybe_window_move(window, seat, serial, trigger, None));
                            } else if button == BTN_RIGHT {
                                let window = window.clone();
                                let seat = seat.clone();
                                let location = pointer_loc.to_i32_round() - self.decorations_offset();
                                state.core.handle.insert_idle(move |state| {
                                    state.pop_up_window_menu(&window, &seat, serial, ActionLocation::WindowRelative(location));
                                });
                                // XXX: not bothering with a persistent pressed state for the menu button; I'm not
                                // sure this is actually the right thing to do.
                                self.pressed_state = PressedState::None;
                            }
                        } else if let Ok(edges) = ResizeEdge::try_from(new_pressed_state) {
                            state
                                .core
                                .handle
                                .insert_idle(move |state| state.start_maybe_window_resize(window, seat, serial, edges, trigger, None));
                        }
                    }
                }
            }
        }
    }

    pub fn button_press<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        button: u32,
        serial: Serial,
    ) {
        self.button_press_or_touch_down(seat, state, window, button, serial, GrabTrigger::Pointer);
    }

    pub fn touch_down<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        button: u32,
        serial: Serial,
    ) {
        self.button_press_or_touch_down(seat, state, window, button, serial, GrabTrigger::Touch);
    }

    pub fn button_release<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        button: u32,
        _serial: Serial,
        time: u32,
    ) {
        tracing::debug!("got button release");

        if self.pressed_state != PressedState::None {
            tracing::debug!("got release from active pressed state");

            if let Some(pointer_loc) = self.pointer_loc {
                let (layout, scale) = self.hit_test_layout(state, window, pointer_loc);
                let pointer_loc_physical = pointer_loc.to_physical(scale);

                let buttons = [
                    (&layout.close, PressedState::Close),
                    (&layout.hide, PressedState::Hide),
                    (&layout.maximize, PressedState::Maximize),
                    (&layout.menu, PressedState::Menu),
                    (&layout.shade, PressedState::Shade),
                    (&layout.stick, PressedState::Stick),
                    (&layout.titlebar, PressedState::Titlebar),
                ];

                let final_pressed_state = buttons
                    .iter()
                    .find_map(|(rect, flag)| point_in_rect(rect, pointer_loc_physical).then_some(*flag));
                let final_pressed_state = final_pressed_state.unwrap_or(PressedState::None);

                if final_pressed_state == self.pressed_state {
                    // Use an idle function for many because we otherwise end up recursively trying
                    // to borrow the RefCell that WindowDecorations (aka 'self') is in, and crash.
                    match final_pressed_state {
                        PressedState::None => (),
                        PressedState::Hide => {
                            let window = window.clone();
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_minimized(&window);
                            });
                        }
                        PressedState::Menu => (), // We pop up the menu on press
                        PressedState::Close => state.close_window(window),
                        PressedState::Shade => {
                            let window = window.clone();
                            let is_shaded = !self.button_toggled_states.contains(ButtonToggledStates::Shade);
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_shaded(&window, is_shaded);
                            });
                        }
                        PressedState::Stick => {
                            let window = window.clone();
                            let new_is_sticky = !self.button_toggled_states.contains(ButtonToggledStates::Stick);
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_sticky(&window, new_is_sticky);
                            });
                        }
                        PressedState::Maximize => {
                            let window = window.clone();
                            let new_is_maximized = !self.button_toggled_states.contains(ButtonToggledStates::Maximize);
                            state.core.handle.insert_idle(move |state| {
                                if new_is_maximized {
                                    state.set_window_maximized(&window, None);
                                } else {
                                    state.set_window_unmaximized(&window, None);
                                }
                            });
                        }
                        PressedState::Titlebar => self.handle_titlebar_double_click(state, window, button, time, pointer_loc),
                        _ => (),
                    }

                    if final_pressed_state != PressedState::Titlebar {
                        self.titlebar_double_click_state = None;
                    }
                }
            }

            if is_button_pressed(self.pressed_state) {
                self.pressed_state = PressedState::None;
                self.invalidate_render_state(DirtyFlags::TITLEBAR);
            } else {
                self.pressed_state = PressedState::None;
            }
        } else {
            self.titlebar_double_click_state = None;
        }
    }

    fn handle_titlebar_double_click<BackendData: Backend>(
        &mut self,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        button: u32,
        time: u32,
        pointer_loc: Point<f64, Logical>,
    ) {
        let (layout, scale) = self.hit_test_layout(state, window, pointer_loc);
        let pointer_loc_physical = pointer_loc.to_physical(scale);
        let other_parts_to_ignore = [&layout.top_left, &layout.top, &layout.top_right];

        let double_click_action = self.config.double_click_action();
        if double_click_action != DoubleClickAction::None
            && button == BTN_LEFT
            && !other_parts_to_ignore.iter().any(|part| point_in_rect(part, pointer_loc_physical))
        {
            if let Some(dc_state) = &mut self.titlebar_double_click_state {
                let distance = {
                    let dx = dc_state.last_location.x - pointer_loc.x;
                    let dy = dc_state.last_location.y - pointer_loc.y;
                    (dx * dx + dy * dy).sqrt()
                };
                let elapsed = Duration::from_millis(time as u64).saturating_sub(Duration::from_millis(dc_state.last_time_msec as u64));

                if distance <= state.core.double_click_distance && elapsed <= state.core.double_click_time {
                    match double_click_action {
                        DoubleClickAction::Hide => state.set_window_minimized(window),
                        DoubleClickAction::Shade => state.set_window_shaded(window, !window.shaded()),
                        DoubleClickAction::Above => {
                            if window.always_on_top() {
                                state.set_window_normal_stacking(window);
                            } else {
                                state.set_window_always_on_top(window);
                            }
                        }
                        DoubleClickAction::Maximize => {
                            // Use an idle function here because we otherwise end up recursively trying
                            // to borrow the RefCell that WindowDecorations (aka 'self') is in, and
                            // crash.
                            let window = window.clone();
                            state.core.handle.insert_idle(move |state| {
                                if !window.maximized() {
                                    state.set_window_maximized(&window, None);
                                } else {
                                    state.set_window_unmaximized(&window, None);
                                }
                            });
                        }
                        DoubleClickAction::Fill => {
                            // Use an idle function here because we otherwise end up recursively trying
                            // to borrow the RefCell that WindowDecorations (aka 'self') is in, and
                            // crash.
                            let window = window.clone();
                            state.core.handle.insert_idle(move |state| {
                                state.fill_window(&window, FillMode::Both);
                            });
                        }
                        DoubleClickAction::None => (),
                    }
                } else {
                    dc_state.last_location = pointer_loc;
                    dc_state.last_time_msec = time;
                }
            } else {
                self.titlebar_double_click_state = Some(DoubleClickState {
                    last_location: pointer_loc,
                    last_time_msec: time,
                });
            }
        } else {
            self.titlebar_double_click_state = None;
        }
    }

    pub fn pointer_axis<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        _time: u32,
        axis: (f64, f64),
    ) {
        if self.hover_state == HoverState::Titlebar && state.core.config.mousewheel_rollup() {
            let steps = self.scroll_accumulator.accumulate(axis.1);
            if steps != 0 {
                let window = window.clone();
                state.core.handle.insert_idle(move |state| {
                    state.set_window_shaded(&window, steps < 0);
                });
            }
        } else {
            self.scroll_accumulator.reset();
        }
    }

    pub fn update_theme(&mut self, decoration_theme: &DecorationTheme) {
        if self.decoration_theme.theme_id() != decoration_theme.theme_id() {
            if self.config.show_app_icon() && self.config.button_layout().includes(TitlebarButton::Menu) && {
                let old_size = self
                    .decoration_theme
                    .button_texture(DecorButtonName::Menu, DecorButtonState::Active, DecorBackgroundState::Active)
                    .map(|menu| menu.size());
                let new_size = decoration_theme
                    .button_texture(DecorButtonName::Menu, DecorButtonState::Active, DecorBackgroundState::Active)
                    .map(|menu| menu.size());
                new_size.is_some() && old_size != new_size
            } {
                self.render_state.window_icon_pixels = None;
            }
            self.decoration_theme = decoration_theme.clone();
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn icon_theme_updated(&mut self) {
        if self.config.show_app_icon()
            && self.config.button_layout().includes(TitlebarButton::Menu)
            && self
                .window_icon
                .as_ref()
                .is_some_and(|window_icon| matches!(window_icon, ImageData::NamedIcon(_)))
        {
            self.render_state.window_icon_pixels = None;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR);
        }
    }

    pub fn theme_properties_updated(&mut self) {
        let flags = self.recalculate_layout();
        self.invalidate_render_state(flags | DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT);
    }

    pub fn update_font_options(&mut self, font_options: gtk::cairo::FontOptions) {
        self.font_options = font_options;
        let flags = self.recalculate_layout();
        self.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
    }

    pub fn update_window_size(&mut self, window_size: Size<i32, Logical>) {
        if self.window_size != window_size {
            self.window_size = window_size;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags);
        }
    }

    pub fn update_scale(&mut self, scale: OutputScale) {
        let changed = self.scale.integer_scale() != scale.integer_scale() || self.scale.fractional_scale() != scale.fractional_scale();
        if changed {
            self.scale = scale;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn update_window_title(&mut self, window_title: &str) {
        let window_title = Some(window_title.to_owned());
        if self.window_title != window_title {
            self.window_title = window_title;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn update_hide_titlebar_when_maximized(&mut self, hidden: bool) {
        if self.hide_titlebar_when_maximized != hidden {
            self.hide_titlebar_when_maximized = hidden;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn update_app_icon(&mut self, window_icon: Option<ImageData>) {
        if self.config.show_app_icon() && self.config.button_layout().includes(TitlebarButton::Menu) {
            self.window_icon = window_icon;
            self.render_state.window_icon_pixels = None;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR);
        }
    }

    pub fn update_active_state(&mut self, is_active: bool) {
        if self.is_active != is_active {
            self.is_active = is_active;
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn update_maximized_state(&mut self, is_maximized: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Maximize) != is_maximized {
            if is_maximized {
                self.button_toggled_states |= ButtonToggledStates::Maximize;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Maximize;
            }
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn update_is_shaded_state(&mut self, is_shaded: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Shade) != is_shaded {
            if is_shaded {
                self.button_toggled_states |= ButtonToggledStates::Shade;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Shade;
            }
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR);
        }
    }

    fn update_titlebar_blink_state(&mut self, blink_state: TitlebarBlinkState) {
        if self.titlebar_blink_state != blink_state {
            self.titlebar_blink_state = blink_state;
            self.invalidate_render_state(DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT);
        }
    }

    pub fn toggle_titlebar_blink_state(&mut self) {
        let blink_state = match self.titlebar_blink_state {
            TitlebarBlinkState::None => {
                if self.is_active {
                    TitlebarBlinkState::Inactive
                } else {
                    TitlebarBlinkState::Active
                }
            }
            TitlebarBlinkState::Active => TitlebarBlinkState::Inactive,
            TitlebarBlinkState::Inactive => TitlebarBlinkState::Active,
        };
        self.update_titlebar_blink_state(blink_state);
    }

    pub fn disable_titlebar_blink(&mut self) {
        self.update_titlebar_blink_state(TitlebarBlinkState::None);
    }

    pub fn refresh_layout(&mut self) {
        let flags = self.recalculate_layout();
        self.invalidate_render_state(flags);
    }

    pub fn update_is_sticky_state(&mut self, is_sticky: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Stick) != is_sticky {
            if is_sticky {
                self.button_toggled_states |= ButtonToggledStates::Stick;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Stick;
            }
            let flags = self.recalculate_layout();
            self.invalidate_render_state(flags | DirtyFlags::TITLEBAR);
        }
    }

    fn bg_state(&self) -> DecorBackgroundState {
        match self.titlebar_blink_state {
            TitlebarBlinkState::None => {
                if self.is_active {
                    DecorBackgroundState::Active
                } else {
                    DecorBackgroundState::Inactive
                }
            }
            TitlebarBlinkState::Active => DecorBackgroundState::Active,
            TitlebarBlinkState::Inactive => DecorBackgroundState::Inactive,
        }
    }

    fn recalculate_layout(&mut self) -> DirtyFlags {
        profiling::scope!("WindowDecorations::recalculate_layout");
        if self.window_size.w <= 0 || self.window_size.h <= 0 {
            return DirtyFlags::empty();
        }

        let old_titlebar_size = self.layout.titlebar.size;
        self.layout = self.build_layout(self.scale.fractional_scale(), self.title_text_size);
        self.input_region = self.build_input_region(&self.layout);
        if self.layout.shadow_size.is_empty() {
            self.render_state.shadow_cache.clear();
        }
        self.scaled.borrow_mut().clear();

        let mut flags = DirtyFlags::empty();
        if self.layout.titlebar.size != old_titlebar_size {
            flags |= DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT;
        }
        flags
    }

    // Builds a decoration layout in physical pixels for `scale`.  Borders, corners and titlebar
    // height are native theme-bitmap px (scale-independent); only the content span
    // (`window_size × scale`) and hence the titlebar width depend on `scale`.  Pure, so it serves
    // both the canonical `self.layout` (at the decoration scale) and the per-output `ScaledRender`
    // entries (each at its output's render scale).
    fn build_layout(&self, scale: f64, title_text_size: Size<i32, Physical>) -> DecorationLayout {
        let mut layout = DecorationLayout::zeroed();
        if self.window_size.w <= 0 || self.window_size.h <= 0 {
            return layout;
        }

        let bg_state = self.bg_state();
        let is_shaded = self.button_toggled_states.contains(ButtonToggledStates::Shade);
        let borderless_maximize = self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize();
        let titleless_maximize =
            borderless_maximize && (self.hide_titlebar_when_maximized || self.config.titleless_maximize()) && !is_shaded;

        let frame_border_top = self.config.frame_border_top();
        let frame_top_h = match self.decoration_theme.title_background_textures(bg_state) {
            DecorTitleTextures::TitleStretched(texture) => texture.size().to_physical().h,
            DecorTitleTextures::Title5Part { title3, .. } => title3.size().to_physical().h,
        };
        let top_clip = if titleless_maximize {
            frame_top_h
        } else if borderless_maximize {
            match self.decoration_theme.title_background_textures(bg_state) {
                DecorTitleTextures::Title5Part { top3: Some(top3), .. } => top3.size().to_physical().h,
                _ => frame_border_top,
            }
        } else {
            0
        };
        layout.top_clip = top_clip;
        let visible_top_h = (frame_top_h - top_clip).max(0);

        let (
            frame_left_w,
            frame_right_w,
            frame_bottom_h,
            corner_top_left_size,
            corner_top_right_size,
            corner_bottom_left_size,
            corner_bottom_right_size,
        ) = if borderless_maximize {
            (0, 0, 0, Size::new(0, 0), Size::new(0, 0), Size::new(0, 0), Size::new(0, 0))
        } else {
            let bottom_strip = self.decoration_theme.bottom_background_texture(bg_state);
            (
                self.decoration_theme
                    .background_texture(DecorBackgroundName::Left, bg_state)
                    .size()
                    .to_physical()
                    .w,
                self.decoration_theme
                    .background_texture(DecorBackgroundName::Right, bg_state)
                    .size()
                    .to_physical()
                    .w,
                bottom_strip.bottom_extents.size.to_physical().h,
                self.decoration_theme
                    .background_texture(DecorBackgroundName::TopLeft, bg_state)
                    .size()
                    .to_physical(),
                self.decoration_theme
                    .background_texture(DecorBackgroundName::TopRight, bg_state)
                    .size()
                    .to_physical(),
                bottom_strip.bottom_left_extents.size.to_physical(),
                bottom_strip.bottom_right_extents.size.to_physical(),
            )
        };

        let window_size_physical = self.window_size.to_f64().to_physical(scale).to_i32_round::<i32>();
        let total_frame_size = Size::<_, Physical>::new(
            frame_left_w + window_size_physical.w + frame_right_w,
            visible_top_h + frame_bottom_h + if is_shaded { 0 } else { window_size_physical.h },
        );

        let frame_top_size = Size::<_, Physical>::new(
            (total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w).max(0),
            frame_top_h,
        );

        layout.top_left = Rectangle::new((0, 0).into(), corner_top_left_size);
        layout.top_right = Rectangle::new((total_frame_size.w - corner_top_right_size.w, 0).into(), corner_top_right_size);
        layout.top = Rectangle::new(
            (corner_top_left_size.w, 0).into(),
            (
                (total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w).max(0),
                frame_bottom_h, // Make the top resize grip area the same height as the bottom
            )
                .into(),
        );

        layout.bottom_left = Rectangle::new((0, total_frame_size.h - corner_bottom_left_size.h).into(), corner_bottom_left_size);
        layout.bottom_right = Rectangle::new((total_frame_size - corner_bottom_right_size).to_point(), corner_bottom_right_size);
        layout.bottom = Rectangle::new(
            (corner_bottom_left_size.w, total_frame_size.h - frame_bottom_h).into(),
            (
                (total_frame_size.w - corner_bottom_left_size.w - corner_bottom_right_size.w).max(0),
                frame_bottom_h,
            )
                .into(),
        );

        if borderless_maximize {
            layout.left = Rectangle::zero();
            layout.right = Rectangle::zero();
        } else if is_shaded {
            layout.left = Rectangle::new((0, visible_top_h).into(), (frame_left_w, 0).into());
            layout.right = Rectangle::new(
                (total_frame_size.w - frame_right_w, visible_top_h).into(),
                (frame_right_w, 0).into(),
            );
        } else {
            layout.left = Rectangle::new(
                (0, visible_top_h).into(),
                (
                    frame_left_w,
                    (window_size_physical.h + frame_bottom_h - corner_bottom_left_size.h).max(0),
                )
                    .into(),
            );
            layout.right = Rectangle::new(
                (total_frame_size.w - frame_right_w, visible_top_h).into(),
                (
                    frame_right_w,
                    (window_size_physical.h + frame_bottom_h - corner_bottom_right_size.h).max(0),
                )
                    .into(),
            );
        }

        let btn_offset = if self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize() {
            self.config.maximized_offset()
        } else {
            self.config.button_offset()
        };
        let btn_spacing = self.config.button_spacing();

        let mut visible_buttons = HashSet::<TitlebarButton>::new();
        let button_layout = self.config.button_layout();

        let mut btn_x = (frame_left_w + btn_offset).max(0);
        let btn_right = total_frame_size.w - frame_right_w - btn_offset;

        for btn in &button_layout.start {
            let btn_name = DecorButtonName::from((*btn, self.button_toggled_states));
            let btn_state = DecorButtonState::from((*btn, bg_state, self.hover_state, self.pressed_state));
            if let Some(btn_tex) = self.decoration_theme.button_texture(btn_name, btn_state, bg_state) {
                let btn_size = btn_tex.size().to_physical();

                if btn_x + btn_size.w + btn_spacing < btn_right {
                    let extents = Rectangle::new((btn_x, (visible_top_h - btn_size.h + 1) / 2).into(), btn_size);
                    btn_x += btn_size.w + btn_spacing;
                    *Self::button_extents_mut(&mut layout, *btn) = extents;
                    visible_buttons.insert(*btn);
                }
            }
        }

        let btn_left = btn_x + btn_spacing;
        let mut btn_x = total_frame_size.w - frame_right_w + btn_spacing - btn_offset;

        for btn in button_layout.end.iter().rev() {
            let btn_name = DecorButtonName::from((*btn, self.button_toggled_states));
            let btn_state = DecorButtonState::from((*btn, bg_state, self.hover_state, self.pressed_state));
            if let Some(btn_tex) = self.decoration_theme.button_texture(btn_name, btn_state, bg_state) {
                let btn_size = btn_tex.size().to_physical();

                if btn_x - btn_size.w - btn_spacing > btn_left {
                    btn_x -= btn_size.w + btn_spacing;
                    let extents = Rectangle::new((btn_x, (visible_top_h - btn_size.h + 1) / 2).into(), btn_size);
                    *Self::button_extents_mut(&mut layout, *btn) = extents;
                    visible_buttons.insert(*btn);
                }
            }
        }

        for btn in [
            TitlebarButton::Menu,
            TitlebarButton::Hide,
            TitlebarButton::Stick,
            TitlebarButton::Shade,
            TitlebarButton::Close,
            TitlebarButton::Maximize,
        ] {
            if !visible_buttons.contains(&btn) {
                *Self::button_extents_mut(&mut layout, btn) = Rectangle::zero();
            }
        }

        let mut btn_left = btn_left - 2 * btn_spacing;
        let mut btn_right = btn_x;
        if btn_left > btn_right {
            std::mem::swap(&mut btn_left, &mut btn_right);
        }

        if frame_top_size.w > 0 {
            let btn_left = btn_left.max(corner_top_left_size.w);
            let btn_right = btn_right
                .min(total_frame_size.w - corner_top_right_size.w)
                .max(corner_top_left_size.w);

            let btn_left = btn_left - corner_top_left_size.w;
            let btn_right = btn_right - corner_top_left_size.w;

            let mut x = 0;
            let mut hoffset = 0;
            let voffset = if bg_state == DecorBackgroundState::Active {
                self.config.title_vertical_offset_active()
            } else {
                self.config.title_vertical_offset_inactive()
            };

            let title_height = title_text_size.h;
            let mut title_y = voffset + (visible_top_h - title_height) / 2;
            if title_y + title_height > visible_top_h {
                title_y = 0.max(visible_top_h - title_height);
            }

            let title_bg_textures = self.decoration_theme.title_background_textures(bg_state);
            let top_height = if let DecorTitleTextures::Title5Part { top3: Some(top3), .. } = &title_bg_textures {
                top3.size().to_physical().h
            } else if frame_border_top > 0 {
                frame_border_top
            } else {
                (frame_top_h / 10 + 1).min(title_y - 1).max(0)
            };

            let w1;
            let (w2, w4) = if let DecorTitleTextures::Title5Part { title2, title4, .. } = &title_bg_textures {
                (title2.size().to_physical().w, title4.size().to_physical().w)
            } else {
                (0, 0)
            };
            let w3;

            if self.config.full_width_title() {
                w1 = btn_left;
                let w5 = frame_top_size.w - btn_right;
                w3 = (frame_top_size.w - w1 - w2 - w4 - w5).max(0);

                hoffset = match self.config.title_alignment() {
                    TitleAlignment::Left => self.config.title_horizontal_offset(),
                    TitleAlignment::Right => w3 - title_text_size.w - self.config.title_horizontal_offset(),
                    TitleAlignment::Center => (w3 / 2) - (title_text_size.w / 2),
                }
                .max(self.config.title_horizontal_offset());
            } else {
                let title_shadow = if bg_state == DecorBackgroundState::Active {
                    self.config.title_shadow_active()
                } else {
                    self.config.title_shadow_inactive()
                } as i32; // FIXME: this seems wrong
                w3 = (title_text_size.w + title_shadow).min(frame_top_size.w - w2 - w4).max(0);

                w1 = match self.config.title_alignment() {
                    TitleAlignment::Left => btn_left + self.config.title_horizontal_offset(),
                    TitleAlignment::Right => btn_right - w2 - w3 - w4 - self.config.title_horizontal_offset(),
                    TitleAlignment::Center => btn_left + ((btn_right - btn_left) / 2) - (w3 / 2) - w2,
                }
                .max(btn_left);
            }

            match &title_bg_textures {
                DecorTitleTextures::TitleStretched(_) => {
                    if let TitleLayout::Title5Part { .. } = &layout.title {
                        layout.title = TitleLayout::TitleStretched {
                            extents: Rectangle::zero(),
                        };
                    }
                }
                DecorTitleTextures::Title5Part { .. } => {
                    if let TitleLayout::TitleStretched { .. } = &layout.title {
                        layout.title = TitleLayout::Title5Part {
                            title1: Rectangle::zero(),
                            top1: Rectangle::zero(),
                            title2: Rectangle::zero(),
                            top2: Rectangle::zero(),
                            title3: Rectangle::zero(),
                            top3: Rectangle::zero(),
                            title4: Rectangle::zero(),
                            top4: Rectangle::zero(),
                            title5: Rectangle::zero(),
                            top5: Rectangle::zero(),
                        };
                    }
                }
            }

            let title_x;
            match (&title_bg_textures, &mut layout.title) {
                (DecorTitleTextures::TitleStretched(_), TitleLayout::TitleStretched { extents }) => {
                    *extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (frame_top_size.w, visible_top_h).into());

                    title_x = hoffset + w1 + w2;
                    let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                    layout.title_text_max_width = title_max_width;
                    layout.title_text = Rectangle::new(
                        (corner_top_left_size.w + title_x, title_y).into(),
                        (btn_right - w4, visible_top_h).into(),
                    );
                }

                (
                    DecorTitleTextures::Title5Part { .. },
                    TitleLayout::Title5Part {
                        title1: title1_ext,
                        top1: top1_ext,
                        title2: title2_ext,
                        top2: top2_ext,
                        title3: title3_ext,
                        top3: top3_ext,
                        title4: title4_ext,
                        top4: top4_ext,
                        title5: title5_ext,
                        top5: top5_ext,
                    },
                ) => {
                    let visible_top_height = (top_height - top_clip).max(0);

                    if w1 > 0 {
                        *title1_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, visible_top_h).into());
                        *top1_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, visible_top_height).into());
                        x += w1;
                    } else {
                        *title1_ext = Rectangle::zero();
                        *top1_ext = Rectangle::zero();
                    }

                    *title2_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, visible_top_h).into());
                    *top2_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, visible_top_height).into());
                    x += w2;

                    layout.title_text = if w3 > 0 {
                        *title3_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_h).into());
                        *top3_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_height).into());
                        title_x = hoffset + x;
                        x += w3;

                        let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                        layout.title_text_max_width = title_max_width;

                        Rectangle::new(
                            (corner_top_left_size.w + title_x, title_y).into(),
                            (btn_right - w4, visible_top_h).into(),
                        )
                    } else {
                        *title3_ext = Rectangle::zero();
                        *top3_ext = Rectangle::zero();
                        layout.title_text_max_width = 0;
                        Rectangle::zero()
                    };

                    x = x.min(btn_right - w4);
                    *title4_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, visible_top_h).into());
                    *top4_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, visible_top_height).into());
                    x += w4;

                    // Compute the remaining width after all title parts, capped at the right
                    // edge of the frame top.  xfwm4 passes the full frame width to
                    // frameFillTitlePixmap() for title5 and relies on window clipping; we have
                    // to do the arithmetic explicitly.
                    let w5_remaining = (frame_top_size.w - x).max(0);
                    if w5_remaining > 0 {
                        *title5_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5_remaining, visible_top_h).into());
                        *top5_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5_remaining, visible_top_height).into());
                    } else {
                        *title5_ext = Rectangle::zero();
                        *top5_ext = Rectangle::zero();
                    }
                }

                _ => unreachable!(),
            }
        }

        let is_maximized = self.button_toggled_states.contains(ButtonToggledStates::Maximize);
        if self.config.show_frame_shadow() && !is_maximized {
            let sp = ShadowParams::new(
                (self.config.shadow_delta_x(), self.config.shadow_delta_y()).into(),
                self.config.shadow_delta_width(),
                self.config.shadow_delta_height(),
                total_frame_size,
            );
            layout.shadow_offset = sp.offset;
            layout.shadow_size = sp.size;
            layout.shadow_frame_size = total_frame_size;
        } else {
            layout.shadow_offset = Point::default();
            layout.shadow_size = Size::default();
            layout.shadow_frame_size = Size::default();
        }

        layout.titlebar = Rectangle::new((0, 0).into(), (total_frame_size.w, visible_top_h).into());

        layout
    }

    fn invalidate_render_state(&mut self, flags: DirtyFlags) {
        if flags.contains(DirtyFlags::TITLE_TEXT) {
            profiling::scope!("invalidate_title_text");
            let (_, title_extents) = create_title_layout(
                &self.font_map,
                &self.font_options,
                self.window_title.as_deref(),
                &self.config.title_font(),
                self.scale.fractional_scale(),
            );
            self.title_text_size = title_extents.size;
            self.scaled.borrow_mut().clear();
        } else if flags.contains(DirtyFlags::TITLEBAR) {
            for entry in self.scaled.borrow().values() {
                entry.titlebar_texture.replace(None);
            }
        }

        if flags.intersects(DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT) {
            self.render_state
                .load_window_icon(&self.layout.menu, self.window_icon.as_ref(), &self.icon_theme);
            self.render_state.invalidate_titlebar();
        }
    }

    #[inline]
    fn button_extents_mut(layout: &mut DecorationLayout, btn: TitlebarButton) -> &mut Rectangle<i32, Physical> {
        match btn {
            TitlebarButton::Menu => &mut layout.menu,
            TitlebarButton::Hide => &mut layout.hide,
            TitlebarButton::Stick => &mut layout.stick,
            TitlebarButton::Shade => &mut layout.shade,
            TitlebarButton::Close => &mut layout.close,
            TitlebarButton::Maximize => &mut layout.maximize,
            TitlebarButton::SideSeparator => unreachable!(),
        }
    }

    // Builds the decoration input region for `layout`: the union, in frame-physical px, of every
    // visible piece's opaque pixels.  Each piece's native-px opaque region (from its `DecorTexture`)
    // is transformed into the frame by the same placement and tiling/stretching the renderer uses,
    // then all the rects are coalesced via pixman into a compact banded set.  Mirrors xfwm4's
    // XSHAPE frame shape, so clicks in transparent titlebar/border gaps fall through.  Built per
    // scale because the content span shifts the right/bottom pieces (same reason the layout is).
    fn build_input_region(&self, layout: &DecorationLayout) -> Vec<Rectangle<i32, Physical>> {
        let bg_state = self.bg_state();
        let theme = &self.decoration_theme;
        let title_src_offset = Point::<i32, Buffer>::from((0, layout.top_clip));
        let mut boxes = Vec::<pixman::Box32>::new();

        append_texture_region(
            &mut boxes,
            theme.background_texture(DecorBackgroundName::TopLeft, bg_state),
            layout.top_left,
            Point::default(),
        );
        append_texture_region(
            &mut boxes,
            theme.background_texture(DecorBackgroundName::TopRight, bg_state),
            layout.top_right,
            Point::default(),
        );

        match (theme.title_background_textures(bg_state), &layout.title) {
            (DecorTitleTextures::TitleStretched(tex), TitleLayout::TitleStretched { extents }) => {
                append_texture_region(&mut boxes, tex, *extents, title_src_offset);
            }
            (
                DecorTitleTextures::Title5Part {
                    title1,
                    top1,
                    title2,
                    top2,
                    title3,
                    top3,
                    title4,
                    top4,
                    title5,
                    top5,
                },
                TitleLayout::Title5Part {
                    title1: d1,
                    top1: dt1,
                    title2: d2,
                    top2: dt2,
                    title3: d3,
                    top3: dt3,
                    title4: d4,
                    top4: dt4,
                    title5: d5,
                    top5: dt5,
                },
            ) => {
                for (tex, ext) in [(title1, d1), (title2, d2), (title3, d3), (title4, d4), (title5, d5)] {
                    append_texture_region(&mut boxes, tex, *ext, title_src_offset);
                }
                for (maybe_tex, ext) in [(top1, dt1), (top2, dt2), (top3, dt3), (top4, dt4), (top5, dt5)] {
                    if let Some(tex) = maybe_tex {
                        append_texture_region(&mut boxes, tex, *ext, title_src_offset);
                    }
                }
            }
            _ => (),
        }

        // The button region uses the resting (non-hover/press) bitmap so the input region is stable
        // across hover -- hover/press only swap the composited texture, not the layout or region.
        let btn_state = resting_button_state(bg_state);
        for (btn, extents) in [
            (TitlebarButton::Close, &layout.close),
            (TitlebarButton::Hide, &layout.hide),
            (TitlebarButton::Maximize, &layout.maximize),
            (TitlebarButton::Menu, &layout.menu),
            (TitlebarButton::Shade, &layout.shade),
            (TitlebarButton::Stick, &layout.stick),
        ] {
            if !extents.is_empty() {
                let btn_name = DecorButtonName::from((btn, self.button_toggled_states));
                if let Some(tex) = theme.button_texture(btn_name, btn_state, bg_state) {
                    append_texture_region(&mut boxes, tex, *extents, Point::default());
                }
            }
        }

        append_texture_region(
            &mut boxes,
            theme.background_texture(DecorBackgroundName::Left, bg_state),
            layout.left,
            Point::default(),
        );
        append_texture_region(
            &mut boxes,
            theme.background_texture(DecorBackgroundName::Right, bg_state),
            layout.right,
            Point::default(),
        );

        // The bottom strip is one offscreen-composited texture, but its three source pieces carry
        // their own opaque regions: corners drawn AsIs (band coords), the middle tiled horizontally.
        let bottom = theme.bottom_background_texture(bg_state);
        append_placed_region(
            &mut boxes,
            bottom.bottom_left_opaque,
            bottom.bottom_left_extents.size,
            DecorRenderingMode::AsIs,
            layout.bottom_left,
            Point::default(),
        );
        append_placed_region(
            &mut boxes,
            bottom.bottom_opaque,
            bottom.bottom_extents.size,
            DecorRenderingMode::Tiled(Direction::Horizontal),
            layout.bottom,
            Point::default(),
        );
        append_placed_region(
            &mut boxes,
            bottom.bottom_right_opaque,
            bottom.bottom_right_extents.size,
            DecorRenderingMode::AsIs,
            layout.bottom_right,
            Point::default(),
        );

        pixman::Region32::init_rects(&boxes)
            .rectangles()
            .iter()
            .map(|b| Rectangle::new((b.x1, b.y1).into(), ((b.x2 - b.x1).max(0), (b.y2 - b.y1).max(0)).into()))
            .collect()
    }

    // Builds a per-output render entry at `scale`: the layout, the title text rasterized at that
    // scale, and the icon position within that layout's menu button.  The titlebar texture is
    // composited lazily on first render (and after appearance-only invalidation).
    fn build_scaled_render(&self, scale: f64) -> ScaledRender {
        let (pango_layout, title_extents) = create_title_layout(
            &self.font_map,
            &self.font_options,
            self.window_title.as_deref(),
            &self.config.title_font(),
            scale,
        );
        let layout = self.build_layout(scale, title_extents.size);
        let input_region = self.build_input_region(&layout);
        let title_text_pixels = render_title_text_pixels(
            pango_layout,
            title_extents,
            layout.title_text_max_width as f64,
            &self.config,
            self.bg_state(),
        );
        let window_icon_extents = icon_extents_for(&layout.menu, self.render_state.window_icon_pixels.as_ref());
        ScaledRender {
            layout,
            input_region,
            title_text_pixels,
            window_icon_extents,
            titlebar_texture: RefCell::new(None),
        }
    }
}

render_elements! {
    pub DecorationRenderElement<=GlesRenderer>;
    Texture=TextureRenderElement<GlesTexture>,
    TiledTexture=TextureShaderElement,
}

impl std::fmt::Debug for DecorationRenderElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Texture(arg0) => f.debug_tuple("Texture").field(arg0).finish(),
            Self::TiledTexture(arg0) => f.debug_tuple("TiledTexture").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl AsRenderElements<GlesRenderer> for WindowDecorations {
    type RenderElement = DecorationRenderElement;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut GlesRenderer,
        location: Point<i32, smithay::utils::Physical>,
        scale: smithay::utils::Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        profiling::scope!("WindowDecorations::render_elements");
        let location = location.to_f64();
        let buffer_scale = 1;
        let alpha = alpha * (self.config.frame_opacity() as f32 / 100.).clamp(0., 1.);
        let bg_state = self.bg_state();

        let tiling_shader = self.decoration_theme.tiling_shader();

        // Decorations are native px on every output; only the content region scales.  Each output
        // renders from a `ScaledRender` built at its own render scale, looked up by the `scale`
        // that `render_elements` is handed -- so a window straddling two outputs draws a crisp,
        // correctly-sized titlebar on both halves rather than one decor-scale texture resampled
        // onto the other.
        let key = ScaleKey::new(scale.x);
        let needs_build = !self.scaled.borrow().contains_key(&key);
        if needs_build {
            let entry = self.build_scaled_render(key.scale());
            self.scaled.borrow_mut().insert(key, entry);
        }
        let scaled = self.scaled.borrow();

        if let Some(entry) = scaled.get(&key) {
            if entry.titlebar_texture.borrow().is_none() && !entry.layout.titlebar.is_empty() {
                match DecorationRenderState::composite_titlebar(
                    renderer,
                    bg_state,
                    tiling_shader,
                    &entry.layout,
                    &self.decoration_theme,
                    self.button_toggled_states,
                    self.hover_state,
                    self.pressed_state,
                    entry.title_text_pixels.as_ref(),
                    self.render_state.window_icon_pixels.as_ref(),
                    entry.window_icon_extents,
                ) {
                    Ok(texture) => *entry.titlebar_texture.borrow_mut() = texture,
                    Err(err) => tracing::warn!("Failed to composite titlebar: {err}"),
                }
            }

            // Content-anchored physical edge grid, relative to `location`.  Border thicknesses are
            // native px (`decorations_extents_physical`), the content span is `window_size × scale`
            // exactly as `element.rs` places the client surface, and the outer edges add the native
            // border widths.  Every piece below is positioned from these shared edges via
            // `place_piece` (which inverts smithay's element rounding), so the pieces tile with each
            // other and sit flush against the content.
            let is_shaded = self.button_toggled_states.contains(ButtonToggledStates::Shade);
            let ext = self.decorations_extents_physical();
            let content_left = ext.left;
            let content_top = ext.top;
            let content_right = content_left + (self.window_size.w as f64 * scale.x).round() as i32;
            let content_bottom = if is_shaded {
                content_top
            } else {
                content_top + (self.window_size.h as f64 * scale.y).round() as i32
            };
            let outer_right = content_right + ext.right;
            let outer_bottom = content_bottom + ext.bottom;
            let bottom_strip_h = entry
                .layout
                .bottom_left
                .size
                .h
                .max(entry.layout.bottom.size.h)
                .max(entry.layout.bottom_right.size.h);
            let bottom_band_top = outer_bottom - bottom_strip_h;
            let rel = |loc: (i32, i32), size: (i32, i32)| Rectangle::<i32, Physical>::new(loc.into(), size.into());

            let titlebar_elem = {
                let tex = entry.titlebar_texture.borrow();
                if let Some(tex) = tex.as_ref()
                    && !entry.layout.titlebar.is_empty()
                {
                    let (titlebar_location, render_size) = place_piece(rel((0, 0), (outer_right, content_top)), location, scale);
                    let tex_src = Rectangle::from_size((tex.size().w, tex.size().h).into()).to_f64();
                    vec![DecorationRenderElement::Texture(TextureRenderElement::from_static_texture(
                        self.render_state.titlebar_id.clone(),
                        renderer.context_id(),
                        titlebar_location,
                        tex.clone(),
                        buffer_scale,
                        Transform::Normal,
                        Some(alpha),
                        Some(tex_src),
                        Some(render_size),
                        None,
                        Kind::Unspecified,
                    ))]
                } else {
                    Vec::new()
                }
            };

            let context_id = renderer.context_id();

            let bottom_strip_elem = if bottom_strip_h > 0 {
                let bottom_strip = self.decoration_theme.bottom_background_texture(bg_state);
                let target = rel((0, bottom_band_top), (outer_right, outer_bottom - bottom_band_top));
                let (bottom_strip_location, render_size) = place_piece(target, location, scale);
                vec![DecorationRenderElement::TiledTexture(create_tiled_texture_elem_with_margin(
                    &context_id,
                    self.render_state.bottom_id.clone(),
                    bottom_strip.texture,
                    tiling_shader,
                    bottom_strip_location,
                    render_size,
                    target.size,
                    buffer_scale,
                    alpha,
                    Direction::Horizontal,
                    None,
                    bottom_strip.bottom_left_extents.size.w,
                    bottom_strip.bottom_right_extents.size.w,
                ))]
            } else {
                Vec::new()
            };

            let shadow_elem = if !entry.layout.shadow_size.is_empty() {
                profiling::scope!("ensure_shadow_texture");
                self.render_state
                    .ensure_shadow_texture(renderer, &self.config, entry.layout.shadow_frame_size);
                let shadow_key = ShadowKey::from_config(&self.config, entry.layout.shadow_frame_size);
                if let Some(shadow_tex) = self.render_state.shadow_cache.get(shadow_key) {
                    let shadow_location = location + shadow_tex.offset.to_f64();
                    vec![DecorationRenderElement::Texture(shadow_tex.render_element(
                        renderer,
                        shadow_location,
                        scale,
                        alpha,
                    ))]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let side_height = (bottom_band_top - content_top).max(0);
            let left_target = if entry.layout.left.is_empty() {
                Rectangle::zero()
            } else {
                rel((0, content_top), (content_left, side_height))
            };
            let right_target = if entry.layout.right.is_empty() {
                Rectangle::zero()
            } else {
                rel((content_right, content_top), (outer_right - content_right, side_height))
            };

            [
                titlebar_elem,
                create_render_elem(
                    &context_id,
                    tiling_shader,
                    self.decoration_theme.background_texture(DecorBackgroundName::Left, bg_state),
                    &self.render_state.left_id,
                    left_target,
                    location,
                    buffer_scale,
                    scale,
                    alpha,
                    None,
                ),
                create_render_elem(
                    &context_id,
                    tiling_shader,
                    self.decoration_theme.background_texture(DecorBackgroundName::Right, bg_state),
                    &self.render_state.right_id,
                    right_target,
                    location,
                    buffer_scale,
                    scale,
                    alpha,
                    None,
                ),
                bottom_strip_elem,
                shadow_elem,
            ]
            .into_iter()
            .flatten()
            .map(C::from)
            .collect()
        } else {
            Vec::new()
        }
    }
}

impl WindowElement {
    pub fn decoration_state(&self) -> Ref<'_, WindowState> {
        self.user_data()
            .get_or_insert(|| RefCell::new(WindowState { window_decorations: None }))
            .borrow()
    }

    pub fn decoration_state_mut(&self) -> RefMut<'_, WindowState> {
        self.user_data()
            .get_or_insert(|| RefCell::new(WindowState { window_decorations: None }))
            .borrow_mut()
    }

    #[allow(clippy::too_many_arguments)]
    fn enable_decorations(
        &self,
        window_size: Size<i32, Logical>,
        window_icon: Option<ImageData>,
        scale: OutputScale,
        config: &Xfwl4Config,
        decoration_theme: &DecorationTheme,
        icon_theme: &FreedesktopIconsIconTheme,
        font_map: &pango::FontMap,
        font_options: &cairo::FontOptions,
    ) {
        let mut decoration_state = self.decoration_state_mut();
        if decoration_state.window_decorations.is_none() {
            let window_title = match self.0.underlying_surface() {
                WindowSurface::Wayland(toplevel_surface) => window_title_for_xdg_toplevel(toplevel_surface),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(x11_surface) => Some(x11_surface.title()),
            };
            let hide_titlebar_when_maximized = self.props().hide_titlebar_when_maximized;

            decoration_state.window_decorations = Some(WindowDecorations::new(
                window_size,
                window_title,
                hide_titlebar_when_maximized,
                window_icon,
                scale,
                config.clone(),
                decoration_theme.clone(),
                icon_theme.clone(),
                font_map.clone(),
                font_options.clone(),
            ));
        }
    }

    fn disable_decorations(&self) {
        self.decoration_state_mut().window_decorations = None;
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn enable_decorations_for_window(&mut self, window: &WindowElement) {
        let window_size = match window.0.underlying_surface() {
            WindowSurface::Wayland(_) => SpaceElement::geometry(&window.0).size,
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => self.x11_window_content_size(surface),
        };

        let scale = self.core.workspace_manager.decorations_scale_for_window(window);
        let window_icon = match window.0.underlying_surface() {
            WindowSurface::Wayland(toplevel_surface) => {
                let app_info = desktop_app_info_for_xdg_toplevel(toplevel_surface);
                icon_for_xdg_toplevel(toplevel_surface, scale.integer_scale(), app_info.as_ref())
                    .and_then(|icon| self.window_icon_to_image_data(&icon).ok())
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => self.window_icon_for_x11_window(x11_surface),
        };

        window.enable_decorations(
            window_size,
            window_icon,
            scale,
            &self.core.config,
            self.core.decoration_theme.as_ref().unwrap(),
            &self.core.icon_theme,
            &self.core.font_map,
            &self.core.font_options,
        );

        #[cfg(feature = "xwayland")]
        {
            self.x11_update_window_frame_extents(window);
            self.x11_update_window_allowed_actions(window);
        }
    }

    pub(in crate::core) fn disable_decorations_for_window(&self, window: &WindowElement) {
        window.disable_decorations();
        #[cfg(feature = "xwayland")]
        {
            self.x11_update_window_frame_extents(window);
            self.x11_update_window_allowed_actions(window);
        }
    }
}

/// For one axis, computes the f64 element location and the logical (integer) size to feed a render
/// element so that smithay renders it to exactly the physical span `[near, far]`.  smithay derives
/// the element's near edge as `round(location)` and its far edge as `round(location + size·scale)`,
/// both from the f64 location (see `TextureRenderElement::geometry`), so we pick the integer size
/// closest to the span and nudge the f64 location by the residual to land both edges exactly.  The
/// residual is bounded by `scale/2`; clamping to just under ½ keeps `round(location) == near` while
/// still moving the far edge onto `far` (valid whenever `scale < 2`, which covers fractional scales).
fn invert_axis(near: i32, far: i32, scale: f64) -> (f64, i32) {
    let extent = (far - near) as f64;
    let size = (extent / scale).round() as i32;
    let residual = extent - size as f64 * scale;
    (near as f64 + residual.clamp(-0.4999, 0.4999), size)
}

/// Places a decoration piece so smithay renders it to exactly `target` physical pixels, offset from
/// `origin`.  Every piece is positioned from a shared set of physical edge coordinates, so adjacent
/// pieces share edges (no gaps/overlaps) and the content-facing edges sit flush on the client
/// surface, which is positioned by the same edges.
fn place_piece(
    target: Rectangle<i32, Physical>,
    origin: Point<f64, Physical>,
    scale: Scale<f64>,
) -> (Point<f64, Physical>, Size<i32, Logical>) {
    let (lx, sw) = invert_axis(target.loc.x, target.loc.x + target.size.w, scale.x);
    let (ly, sh) = invert_axis(target.loc.y, target.loc.y + target.size.h, scale.y);
    (
        origin + Point::<f64, Physical>::from((lx, ly)),
        Size::<i32, Logical>::from((sw, sh)),
    )
}

fn physical_box(rect: Rectangle<i32, Physical>) -> pixman::Box32 {
    pixman::Box32 {
        x1: rect.loc.x,
        y1: rect.loc.y,
        x2: rect.loc.x + rect.size.w,
        y2: rect.loc.y + rect.size.h,
    }
}

// Clips `region_box` to `bounds`, returning the intersection if non-empty (pixman boxes are
// x2/y2-exclusive).
fn clip_box(region_box: pixman::Box32, bounds: pixman::Box32) -> Option<pixman::Box32> {
    let x1 = region_box.x1.max(bounds.x1);
    let y1 = region_box.y1.max(bounds.y1);
    let x2 = region_box.x2.min(bounds.x2);
    let y2 = region_box.y2.min(bounds.y2);
    (x1 < x2 && y1 < y2).then_some(pixman::Box32 { x1, y1, x2, y2 })
}

// Remaps `opaque_box`, an opaque rect lying within the texture's sampled region `src_rect`, into the
// frame rectangle `dst_rect` the renderer blits that region onto -- scaling each axis independently.
// Near edges round down and far edges round up, so the result always covers `opaque_box`'s pixels.
fn remap_box(opaque_box: Rectangle<i32, Buffer>, src_rect: Rectangle<i32, Buffer>, dst_rect: Rectangle<i32, Physical>) -> pixman::Box32 {
    let map_x = |x: i32| dst_rect.loc.x as f64 + (x - src_rect.loc.x) as f64 * dst_rect.size.w as f64 / src_rect.size.w as f64;
    let map_y = |y: i32| dst_rect.loc.y as f64 + (y - src_rect.loc.y) as f64 * dst_rect.size.h as f64 / src_rect.size.h as f64;
    pixman::Box32 {
        x1: map_x(opaque_box.loc.x).floor() as i32,
        y1: map_y(opaque_box.loc.y).floor() as i32,
        x2: map_x(opaque_box.loc.x + opaque_box.size.w).ceil() as i32,
        y2: map_y(opaque_box.loc.y + opaque_box.size.h).ceil() as i32,
    }
}

// The destination rectangles a `tile`-sized texture lands on when repeated along `direction` to
// cover `placement` (the cross axis keeps the placement's extent, matching how the layout sizes a
// tiled piece to its texture's native cross size).  AsIs/Stretched pieces use `placement` itself.
fn tile_footprints(tile: Size<i32, Buffer>, placement: Rectangle<i32, Physical>, direction: Direction) -> Vec<Rectangle<i32, Physical>> {
    let (tile_extent, span) = match direction {
        Direction::Horizontal => (tile.w, placement.size.w),
        Direction::Vertical => (tile.h, placement.size.h),
    };
    if tile_extent <= 0 {
        Vec::new()
    } else {
        let tile_count = (span + tile_extent - 1) / tile_extent;
        (0..tile_count)
            .map(|index| {
                let offset = index * tile_extent;
                match direction {
                    Direction::Horizontal => Rectangle::new(
                        (placement.loc.x + offset, placement.loc.y).into(),
                        (tile_extent, placement.size.h).into(),
                    ),
                    Direction::Vertical => Rectangle::new(
                        (placement.loc.x, placement.loc.y + offset).into(),
                        (placement.size.w, tile_extent).into(),
                    ),
                }
            })
            .collect()
    }
}

// Appends `opaque_regions` (a texture's native-px opaque rects) into `out` as frame-physical boxes,
// transforming each the same way the renderer transforms the texture's pixels into `placement`:
// AsIs/Stretched blit the sampled region across the placement; Tiled repeats it, one footprint per
// tile.  `src_offset` is the renderer's source crop (e.g. the clipped top of a maximized titlebar),
// so the sampled region runs from `src_offset` to the texture's far edge.
fn append_placed_region(
    out: &mut Vec<pixman::Box32>,
    opaque_regions: &[Rectangle<i32, Buffer>],
    tex_size: Size<i32, Buffer>,
    mode: DecorRenderingMode,
    placement: Rectangle<i32, Physical>,
    src_offset: Point<i32, Buffer>,
) {
    let sampled = Rectangle::new(src_offset, (tex_size.w - src_offset.x, tex_size.h - src_offset.y).into());
    if !placement.is_empty() && sampled.size.w > 0 && sampled.size.h > 0 {
        let footprints = match mode {
            DecorRenderingMode::AsIs | DecorRenderingMode::Stretched(_) => vec![placement],
            DecorRenderingMode::Tiled(direction) => tile_footprints(sampled.size, placement, direction),
        };
        let bounds = physical_box(placement);
        for footprint in footprints {
            for opaque in opaque_regions {
                out.extend(clip_box(remap_box(*opaque, sampled, footprint), bounds));
            }
        }
    }
}

fn append_texture_region(
    out: &mut Vec<pixman::Box32>,
    texture: &DecorTexture,
    placement: Rectangle<i32, Physical>,
    src_offset: Point<i32, Buffer>,
) {
    append_placed_region(
        out,
        texture.opaque_regions(),
        texture.size(),
        texture.rendering_mode(),
        placement,
        src_offset,
    );
}

#[allow(clippy::too_many_arguments)]
fn create_render_elem(
    context_id: &ContextId<GlesTexture>,
    tiling_shader: &GlesTexProgram,
    texture: &DecorTexture,
    id: &Id,
    target: Rectangle<i32, Physical>,
    origin: Point<f64, Physical>,
    buffer_scale: i32,
    scale: Scale<f64>,
    alpha: f32,
    src_offset: Option<Point<i32, Buffer>>,
) -> Vec<DecorationRenderElement> {
    if target.is_empty() {
        vec![]
    } else {
        let (location, render_size) = place_piece(target, origin, scale);
        let geo_size = target.size;
        vec![match texture.rendering_mode() {
            DecorRenderingMode::Tiled(direction) => DecorationRenderElement::TiledTexture(create_tiled_texture_elem(
                context_id,
                id.clone(),
                texture,
                tiling_shader,
                location,
                render_size,
                geo_size,
                buffer_scale,
                alpha,
                direction,
                src_offset,
            )),
            DecorRenderingMode::Stretched(_) => DecorationRenderElement::Texture(create_texture_elem(
                context_id,
                id.clone(),
                texture,
                location,
                render_size,
                buffer_scale,
                alpha,
                src_offset,
            )),
            DecorRenderingMode::AsIs => DecorationRenderElement::Texture(create_texture_elem(
                context_id,
                id.clone(),
                texture,
                location,
                render_size,
                buffer_scale,
                alpha,
                src_offset,
            )),
        }]
    }
}

#[allow(clippy::too_many_arguments)]
fn create_texture_elem(
    context_id: &ContextId<GlesTexture>,
    id: Id,
    texture: &GlesTexture,
    location: Point<f64, Physical>,
    render_size: Size<i32, Logical>,
    buffer_scale: i32,
    alpha: f32,
    src_offset: Option<Point<i32, Buffer>>,
) -> TextureRenderElement<GlesTexture> {
    let tex_size = texture.size();
    let src = if let Some(offset) = src_offset {
        let src_size = Size::<i32, Buffer>::new((tex_size.w - offset.x).max(0), (tex_size.h - offset.y).max(0));
        Rectangle::new(offset, src_size)
    } else {
        Rectangle::new(Point::default(), tex_size)
    }
    .to_logical(buffer_scale, Transform::Normal, &tex_size)
    .to_f64();
    TextureRenderElement::from_static_texture(
        id,
        context_id.clone(),
        location,
        texture.clone(),
        buffer_scale,
        Transform::Normal,
        Some(alpha),
        Some(src),
        Some(render_size),
        None,
        Kind::Unspecified,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_tiled_texture_elem(
    context_id: &ContextId<GlesTexture>,
    id: Id,
    texture: &GlesTexture,
    shader: &GlesTexProgram,
    location: Point<f64, Physical>,
    render_size: Size<i32, Logical>,
    geo_size: Size<i32, Physical>,
    buffer_scale: i32,
    alpha: f32,
    direction: Direction,
    src_offset: Option<Point<i32, Buffer>>,
) -> TextureShaderElement {
    create_tiled_texture_elem_with_margin(
        context_id,
        id,
        texture,
        shader,
        location,
        render_size,
        geo_size,
        buffer_scale,
        alpha,
        direction,
        src_offset,
        0,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_tiled_texture_elem_with_margin(
    context_id: &ContextId<GlesTexture>,
    id: Id,
    texture: &GlesTexture,
    shader: &GlesTexProgram,
    location: Point<f64, Physical>,
    render_size: Size<i32, Logical>,
    geo_size: Size<i32, Physical>,
    buffer_scale: i32,
    alpha: f32,
    direction: Direction,
    src_offset: Option<Point<i32, Buffer>>,
    margin_left: i32,
    margin_right: i32,
) -> TextureShaderElement {
    let element = create_texture_elem(context_id, id, texture, location, render_size, buffer_scale, alpha, src_offset);

    // The tiling shader works in texture (native) pixels, so geo_size is the destination size in
    // physical pixels — not the logical render size, which differs at fractional scale.
    let tex_size = texture.size().to_f64();
    let geo_size = geo_size.to_f64();

    let tile_mask = match direction {
        Direction::Horizontal => (1.0f32, 0.0f32),
        Direction::Vertical => (0.0f32, 1.0f32),
    };

    let uniforms = [
        Uniform::new("tex_size", UniformValue::_2f(tex_size.w as f32, tex_size.h as f32)),
        Uniform::new("geo_size", UniformValue::_2f(geo_size.w as f32, geo_size.h as f32)),
        Uniform::new("tile_mask", UniformValue::_2f(tile_mask.0, tile_mask.1)),
        Uniform::new("margin_left", UniformValue::_1f(margin_left as f32)),
        Uniform::new("margin_right", UniformValue::_1f(margin_right as f32)),
    ]
    .to_vec();

    TextureShaderElement::new(element, shader.clone(), uniforms)
}

// The button's resting (un-hovered, un-pressed) texture for a background state -- the one whose
// opaque region defines the stable clickable area, independent of prelight/pressed appearance.
fn resting_button_state(bg_state: DecorBackgroundState) -> DecorButtonState {
    match bg_state {
        DecorBackgroundState::Active => DecorButtonState::Active,
        DecorBackgroundState::Inactive => DecorButtonState::Inactive,
    }
}

fn is_button_hover(state: HoverState) -> bool {
    matches!(
        state,
        HoverState::Close | HoverState::Hide | HoverState::Maximize | HoverState::Menu | HoverState::Shade | HoverState::Stick
    )
}

fn is_button_pressed(state: PressedState) -> bool {
    matches!(
        state,
        PressedState::Close | PressedState::Hide | PressedState::Maximize | PressedState::Menu | PressedState::Shade | PressedState::Stick
    )
}

impl From<TitlebarButton> for HoverState {
    fn from(button: TitlebarButton) -> Self {
        match button {
            TitlebarButton::Close => Self::Close,
            TitlebarButton::Hide => Self::Hide,
            TitlebarButton::Maximize => Self::Maximize,
            TitlebarButton::Menu => Self::Menu,
            TitlebarButton::Shade => Self::Shade,
            TitlebarButton::Stick => Self::Stick,
            TitlebarButton::SideSeparator => unreachable!(),
        }
    }
}

impl From<TitlebarButton> for PressedState {
    fn from(button: TitlebarButton) -> Self {
        match button {
            TitlebarButton::Close => Self::Close,
            TitlebarButton::Hide => Self::Hide,
            TitlebarButton::Maximize => Self::Maximize,
            TitlebarButton::Menu => Self::Menu,
            TitlebarButton::Shade => Self::Shade,
            TitlebarButton::Stick => Self::Stick,
            TitlebarButton::SideSeparator => unreachable!(),
        }
    }
}

impl From<(TitlebarButton, ButtonToggledStates)> for DecorButtonName {
    fn from((tbtn, toggled): (TitlebarButton, ButtonToggledStates)) -> Self {
        match tbtn {
            TitlebarButton::Menu => Self::Menu,
            TitlebarButton::Hide => Self::Hide,
            TitlebarButton::Close => Self::Close,
            TitlebarButton::Maximize if toggled.contains(ButtonToggledStates::Maximize) => Self::MaximizeToggled,
            TitlebarButton::Maximize => Self::Maximize,
            TitlebarButton::Stick if toggled.contains(ButtonToggledStates::Stick) => Self::StickToggled,
            TitlebarButton::Stick => Self::Stick,
            TitlebarButton::Shade if toggled.contains(ButtonToggledStates::Shade) => Self::ShadeToggled,
            TitlebarButton::Shade => Self::Shade,
            TitlebarButton::SideSeparator => unreachable!(),
        }
    }
}

impl From<(TitlebarButton, DecorBackgroundState, HoverState, PressedState)> for DecorButtonState {
    fn from((tbtn, bg_state, hover, pressed): (TitlebarButton, DecorBackgroundState, HoverState, PressedState)) -> Self {
        let (hover_state, pressed_state) = match tbtn {
            TitlebarButton::Menu => (HoverState::Menu, PressedState::Menu),
            TitlebarButton::Hide => (HoverState::Hide, PressedState::Hide),
            TitlebarButton::Close => (HoverState::Close, PressedState::Close),
            TitlebarButton::Maximize => (HoverState::Maximize, PressedState::Maximize),
            TitlebarButton::Stick => (HoverState::Stick, PressedState::Stick),
            TitlebarButton::Shade => (HoverState::Shade, PressedState::Shade),
            TitlebarButton::SideSeparator => unreachable!(),
        };

        if bg_state == DecorBackgroundState::Inactive {
            DecorButtonState::Inactive
        } else if pressed == pressed_state {
            DecorButtonState::Pressed
        } else if hover == hover_state {
            DecorButtonState::Prelight
        } else {
            DecorButtonState::Active
        }
    }
}

impl TryFrom<PressedState> for ResizeEdge {
    type Error = anyhow::Error;

    fn try_from(value: PressedState) -> Result<Self, Self::Error> {
        match value {
            PressedState::Top => Ok(Self::TOP),
            PressedState::Left => Ok(Self::LEFT),
            PressedState::Right => Ok(Self::RIGHT),
            PressedState::Bottom => Ok(Self::BOTTOM),
            PressedState::TopLeft => Ok(Self::TOP_LEFT),
            PressedState::TopRight => Ok(Self::TOP_RIGHT),
            PressedState::BottomLeft => Ok(Self::BOTTOM_LEFT),
            PressedState::BottomRight => Ok(Self::BOTTOM_RIGHT),
            other => Err(anyhow!("Invalid PressedState {other:?} for resizing")),
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::{allocator::Fourcc, renderer::element::Element};

    use super::*;

    #[derive(Debug, Clone)]
    struct TestTexture;

    impl Texture for TestTexture {
        fn width(&self) -> u32 {
            1
        }

        fn height(&self) -> u32 {
            1
        }

        fn format(&self) -> Option<Fourcc> {
            None
        }
    }

    // `place_piece` works by inverting how smithay's `TextureRenderElement` rounds its geometry
    // (near = round(location), far = round(location + size·scale), both from the f64 location).  If
    // that ever changes, the decoration pieces would silently drift apart again.  This guards
    // against it: for a spread of fractional scales and shared edge coordinates, the element built
    // from `place_piece`'s output must render to exactly the requested physical rectangle.  Hitting
    // each target exactly is what makes adjacent pieces tile and sit flush against the content.
    #[test]
    fn place_piece_hits_exact_physical_rect() {
        let origin = Point::<f64, Physical>::from((0., 0.));
        let xs = [0, 31, 47, 813, 829, 860];
        let ys = [0, 29, 410, 700, 733];

        for &s in &[1.0_f64, 1.25, 1.3, 1.33, 1.5, 1.6, 1.75, 1.9] {
            let scale = Scale::from(s);
            for x in xs.windows(2) {
                for y in ys.windows(2) {
                    let target = Rectangle::<i32, Physical>::new((x[0], y[0]).into(), (x[1] - x[0], y[1] - y[0]).into());
                    let (location, size) = place_piece(target, origin, scale);
                    let element = TextureRenderElement::from_static_texture(
                        Id::new(),
                        ContextId::<TestTexture>::new(),
                        location,
                        TestTexture,
                        1,
                        Transform::Normal,
                        None,
                        None,
                        Some(size),
                        None,
                        Kind::Unspecified,
                    );
                    assert_eq!(element.geometry(scale), target, "scale {s}, target {target:?}");
                }
            }
        }
    }

    fn buffer_rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Buffer> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn physical_rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    // Collects `append_placed_region` output as physical rects, coalesced through pixman exactly as
    // `build_input_region` does, so a test asserts the final hit-test shape rather than raw stamps.
    fn placed_region(
        opaque: &[Rectangle<i32, Buffer>],
        tex_size: Size<i32, Buffer>,
        mode: DecorRenderingMode,
        placement: Rectangle<i32, Physical>,
        src_offset: Point<i32, Buffer>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let mut boxes = Vec::new();
        append_placed_region(&mut boxes, opaque, tex_size, mode, placement, src_offset);
        pixman::Region32::init_rects(&boxes)
            .rectangles()
            .iter()
            .map(|b| Rectangle::new((b.x1, b.y1).into(), (b.x2 - b.x1, b.y2 - b.y1).into()))
            .collect()
    }

    // pixman's Box32 is an FFI struct without PartialEq, so compare as tuples.
    fn box_tuple(b: pixman::Box32) -> (i32, i32, i32, i32) {
        (b.x1, b.y1, b.x2, b.y2)
    }

    #[test]
    fn clip_box_intersects_and_drops_touching_edges() {
        let bounds = pixman::Box32 {
            x1: 0,
            y1: 0,
            x2: 10,
            y2: 10,
        };
        assert_eq!(
            clip_box(
                pixman::Box32 {
                    x1: -5,
                    y1: 2,
                    x2: 5,
                    y2: 20
                },
                bounds
            )
            .map(box_tuple),
            Some((0, 2, 5, 10))
        );
        // Boxes are x2/y2-exclusive, so a box that only touches the far edge is empty.
        assert_eq!(
            clip_box(
                pixman::Box32 {
                    x1: 10,
                    y1: 0,
                    x2: 12,
                    y2: 5
                },
                bounds
            )
            .map(box_tuple),
            None
        );
    }

    #[test]
    fn remap_box_identity_translates() {
        let src = buffer_rect(0, 0, 20, 8);
        let dst = physical_rect(100, 50, 20, 8);
        assert_eq!(box_tuple(remap_box(buffer_rect(3, 2, 4, 5), src, dst)), (103, 52, 107, 57));
    }

    #[test]
    fn remap_box_stretches_and_covers() {
        // Source 10 wide stretched to 20 (2x): the [2,5) span covers [4,10).
        let src = buffer_rect(0, 0, 10, 4);
        let dst = physical_rect(0, 0, 20, 4);
        assert_eq!(box_tuple(remap_box(buffer_rect(2, 0, 3, 4), src, dst)), (4, 0, 10, 4));
    }

    #[test]
    fn tile_footprints_cover_with_partial_last_tile() {
        let placement = physical_rect(5, 7, 25, 4);
        let feet = tile_footprints((10, 4).into(), placement, Direction::Horizontal);
        assert_eq!(
            feet,
            vec![physical_rect(5, 7, 10, 4), physical_rect(15, 7, 10, 4), physical_rect(25, 7, 10, 4)]
        );
    }

    #[test]
    fn placed_region_asis_translates_opaque() {
        let opaque = [buffer_rect(1, 1, 3, 2)];
        let region = placed_region(
            &opaque,
            (8, 6).into(),
            DecorRenderingMode::AsIs,
            physical_rect(100, 40, 8, 6),
            Point::default(),
        );
        assert_eq!(region, vec![physical_rect(101, 41, 3, 2)]);
    }

    #[test]
    fn placed_region_tiles_pattern_and_clips_last_tile() {
        // A 10px-wide tile opaque in [0,4); tiled across a 25px span starts copies at 0/10/20 and
        // clips the last to the placement, then pixman coalesces nothing (the gaps stay).
        let opaque = [buffer_rect(0, 0, 4, 3)];
        let region = placed_region(
            &opaque,
            (10, 3).into(),
            DecorRenderingMode::Tiled(Direction::Horizontal),
            physical_rect(0, 0, 25, 3),
            Point::default(),
        );
        assert_eq!(
            region,
            vec![physical_rect(0, 0, 4, 3), physical_rect(10, 0, 4, 3), physical_rect(20, 0, 4, 3)]
        );
    }

    #[test]
    fn placed_region_src_offset_crops_top() {
        // The renderer samples from y=2 down (top_clip), so the texture's top two rows are dropped
        // and everything below shifts up by two into the placement.
        let opaque = [buffer_rect(0, 0, 5, 6)];
        let region = placed_region(
            &opaque,
            (5, 6).into(),
            DecorRenderingMode::AsIs,
            physical_rect(0, 0, 5, 4),
            Point::from((0, 2)),
        );
        assert_eq!(region, vec![physical_rect(0, 0, 5, 4)]);
    }
}

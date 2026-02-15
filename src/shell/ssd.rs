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

use gtk::{
    cairo,
    gdk::prelude::GdkContextExt,
    pango::{self, traits::FontMapExt},
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Renderer, Texture,
            element::{
                AsRenderElements, Id, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                texture::TextureRenderElement,
            },
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformValue, element::TextureShaderElement},
        },
    },
    desktop::{WindowSurface, space::SpaceElement},
    input::Seat,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform},
    wayland::{compositor, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::warn;

use std::{
    cell::{RefCell, RefMut},
    collections::HashSet,
};

use crate::{
    Xfwl4State,
    backend::Backend,
    config::{TitleAlignment, TitleShadow, TitlebarButton, Xfwl4Config},
    drawing::decorations::{
        DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode, DecorTexture, DecorTitleTextures,
        DecorationTheme, Direction,
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

#[derive(Debug, Clone)]
struct TextureData {
    id: Id,
    extents: Rectangle<i32, Logical>,
}

impl TextureData {
    fn new() -> Self {
        Self {
            id: Id::new(),
            extents: Rectangle::zero(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum TitleTextureData {
    TitleStretched(TextureData),
    Title5Part {
        title1: TextureData,
        top1: TextureData,
        title2: TextureData,
        top2: TextureData,
        title3: TextureData,
        top3: TextureData,
        title4: TextureData,
        top4: TextureData,
        title5: TextureData,
        top5: TextureData,
    },
}

impl TitleTextureData {
    fn new_stretched() -> Self {
        Self::TitleStretched(TextureData::new())
    }

    fn new_5part() -> Self {
        Self::Title5Part {
            title1: TextureData::new(),
            top1: TextureData::new(),
            title2: TextureData::new(),
            top2: TextureData::new(),
            title3: TextureData::new(),
            top3: TextureData::new(),
            title4: TextureData::new(),
            top4: TextureData::new(),
            title5: TextureData::new(),
            top5: TextureData::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ButtonHoverState {
    None,
    Close,
    Hide,
    Maximize,
    Menu,
    Shade,
    Stick,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ButtonPressedState {
    None,
    Close,
    Hide,
    Maximize,
    Menu,
    Shade,
    Stick,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    struct ButtonToggledStates: u8 {
        const Maximize = (1 << 0);
        const Shade = (1 << 1);
        const Stick = (1 << 2);
    }
}

#[derive(Debug, Clone)]
pub struct WindowDecorations {
    pub pointer_loc: Option<Point<f64, Logical>>,
    window_size: Size<i32, Logical>,
    window_title: Option<String>,
    scale: f64,
    config: Xfwl4Config,
    decoration_theme: DecorationTheme,
    font_map: pango::FontMap,
    font_options: cairo::FontOptions,

    is_active: bool,
    button_hover_state: ButtonHoverState,
    button_pressed_state: ButtonPressedState,
    button_toggled_states: ButtonToggledStates,

    title_buffer: MemoryRenderBuffer,
    title_text: TextureData,

    top_left: TextureData,
    title: TitleTextureData,
    top_right: TextureData,
    bottom: TextureData,
    bottom_left: TextureData,
    bottom_right: TextureData,
    left: TextureData,
    right: TextureData,

    close: TextureData,
    hide: TextureData,
    maximize: TextureData,
    menu: TextureData,
    shade: TextureData,
    stick: TextureData,
}

impl WindowDecorations {
    fn new(
        window_size: Size<i32, Logical>,
        window_title: Option<String>,
        scale: f64,
        config: Xfwl4Config,
        decoration_theme: DecorationTheme,
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
            font_map,
            font_options,
            is_active: false,
            button_hover_state: ButtonHoverState::None,
            button_pressed_state: ButtonPressedState::None,
            button_toggled_states: ButtonToggledStates::empty(),
            title_buffer: MemoryRenderBuffer::new(Fourcc::Argb8888, Size::new(1, 1), 1, Transform::Normal, None),
            title_text: TextureData::new(),
            top_left: TextureData::new(),
            top_right: TextureData::new(),
            bottom: TextureData::new(),
            bottom_left: TextureData::new(),
            bottom_right: TextureData::new(),
            left: TextureData::new(),
            right: TextureData::new(),
            title: TitleTextureData::new_stretched(),
            close: TextureData::new(),
            hide: TextureData::new(),
            maximize: TextureData::new(),
            menu: TextureData::new(),
            shade: TextureData::new(),
            stick: TextureData::new(),
        };
        decorations.update();
        decorations
    }

    pub fn point_is_in_decorations(&self, location: Point<f64, Logical>) -> bool {
        let location = location.to_i32_ceil();
        let in_title = match &self.title {
            TitleTextureData::TitleStretched(title) => !title.extents.is_empty() && title.extents.contains(location),
            TitleTextureData::Title5Part {
                title1,
                title2,
                title3,
                title4,
                title5,
                ..
            } => {
                (!title1.extents.is_empty() && title1.extents.contains(location))
                    || (!title2.extents.is_empty() && title2.extents.contains(location))
                    || (!title3.extents.is_empty() && title3.extents.contains(location))
                    || (!title4.extents.is_empty() && title4.extents.contains(location))
                    || (!title5.extents.is_empty() && title5.extents.contains(location))
            }
        };
        if in_title {
            return true;
        }

        for texture_data in [
            &self.top_left,
            &self.top_right,
            &self.bottom_left,
            &self.bottom_right,
            &self.left,
            &self.right,
            &self.bottom,
        ] {
            if !texture_data.extents.is_empty() && texture_data.extents.contains(location) {
                return true;
            }
        }

        false
    }

    pub fn left_decoration_width(&self) -> i32 {
        self.left.extents.size.w
    }

    pub fn right_decoration_width(&self) -> i32 {
        self.right.extents.size.w
    }

    pub fn top_decoration_height(&self) -> i32 {
        match &self.title {
            TitleTextureData::TitleStretched(title) => title.extents.size.h,
            TitleTextureData::Title5Part { title3, .. } => title3.extents.size.h,
        }
    }

    pub fn bottom_decoration_height(&self) -> i32 {
        self.bottom.extents.size.h
    }

    pub fn decorations_offset(&self) -> Point<i32, Logical> {
        (self.left_decoration_width(), self.top_decoration_height()).into()
    }

    pub fn pointer_enter(&mut self, loc: Point<f64, Logical>) {
        self.pointer_loc = Some(loc);
    }

    pub fn pointer_leave(&mut self) {
        self.pointer_loc = None;
    }

    pub fn clicked<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _state: &mut Xfwl4State<BackendData>,
        _window: &WindowElement,
        _serial: Serial,
    ) {
        /*
        match self.pointer_loc.as_ref() {
            Some(loc) if loc.x >= (self.width - BUTTON_WIDTH) as f64 => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => w.send_close(),
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let _ = w.close();
                    }
                };
            }
            Some(loc) if loc.x >= (self.width - (BUTTON_WIDTH * 2)) as f64 => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => state.maximize_request(w.clone()),
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let surface = w.clone();
                        state.handle.insert_idle(move |data| data.maximize_request_x11(&surface));
                    }
                };
            }
            Some(_) => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => {
                        let seat = seat.clone();
                        let toplevel = w.clone();
                        state
                            .handle
                            .insert_idle(move |data| data.move_request_xdg(&toplevel, &seat, serial));
                    }
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let window = w.clone();
                        state.handle.insert_idle(move |data| data.move_request_x11(&window));
                    }
                };
            }
            _ => {}
        };
        */
    }

    pub fn touch_down<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _state: &mut Xfwl4State<BackendData>,
        _window: &WindowElement,
        _serial: Serial,
    ) {
        /*
        match self.pointer_loc.as_ref() {
            Some(loc) if loc.x >= (self.width - BUTTON_WIDTH) as f64 => {}
            Some(loc) if loc.x >= (self.width - (BUTTON_WIDTH * 2)) as f64 => {}
            Some(_) => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => {
                        let seat = seat.clone();
                        let toplevel = w.clone();
                        state
                            .handle
                            .insert_idle(move |data| data.move_request_xdg(&toplevel, &seat, serial));
                    }
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let window = w.clone();
                        state.handle.insert_idle(move |data| data.move_request_x11(&window));
                    }
                };
            }
            _ => {}
        };
        */
    }

    pub fn touch_up<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _state: &mut Xfwl4State<BackendData>,
        _window: &WindowElement,
        _serial: Serial,
    ) {
        /*
        match self.pointer_loc.as_ref() {
            Some(loc) if loc.x >= (self.width - BUTTON_WIDTH) as f64 => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => w.send_close(),
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let _ = w.close();
                    }
                };
            }
            Some(loc) if loc.x >= (self.width - (BUTTON_WIDTH * 2)) as f64 => {
                match window.0.underlying_surface() {
                    WindowSurface::Wayland(w) => state.maximize_request(w.clone()),
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(w) => {
                        let surface = w.clone();
                        state.handle.insert_idle(move |data| data.maximize_request_x11(&surface));
                    }
                };
            }
            _ => {}
        };
        */
    }

    pub fn update_theme(&mut self, decoration_theme: &DecorationTheme) {
        if self.decoration_theme.theme_id() != decoration_theme.theme_id() {
            self.decoration_theme = decoration_theme.clone();
            self.update();
        }
    }

    pub fn theme_properties_updated(&mut self) {
        self.update();
    }

    pub fn update_font_options(&mut self, font_options: gtk::cairo::FontOptions) {
        self.font_options = font_options;
        self.update();
    }

    pub fn update_window_size(&mut self, window_size: Size<i32, Logical>) {
        if self.window_size != window_size {
            self.window_size = window_size;
            self.update();
        }
    }

    pub fn update_window_title(&mut self, window_title: &str) {
        let window_title = Some(window_title.to_owned());
        if self.window_title != window_title {
            self.window_title = window_title;
            self.update();
        }
    }

    pub fn update_active_state(&mut self, is_active: bool) {
        if self.is_active != is_active {
            self.is_active = is_active;
            self.update();
        }
    }

    pub fn update_maximized_state(&mut self, is_maximized: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Maximize) != is_maximized {
            if is_maximized {
                self.button_toggled_states |= ButtonToggledStates::Maximize;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Maximize;
            }
            self.update();
        }
    }

    pub fn update_is_shaded_state(&mut self, is_shaded: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Shade) != is_shaded {
            if is_shaded {
                self.button_toggled_states |= ButtonToggledStates::Shade;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Shade;
            }
            self.update();
        }
    }

    pub fn update_is_sticky_state(&mut self, is_sticky: bool) {
        if self.button_toggled_states.contains(ButtonToggledStates::Stick) != is_sticky {
            if is_sticky {
                self.button_toggled_states |= ButtonToggledStates::Stick;
            } else {
                self.button_toggled_states &= !ButtonToggledStates::Stick;
            }
            self.update();
        }
    }

    fn update(&mut self) {
        if self.window_size.w > 0 && self.window_size.h > 0 {
            let bg_state = if self.is_active {
                DecorBackgroundState::Active
            } else {
                DecorBackgroundState::Inactive
            };

            let frame_border_top = self.config.frame_border_top();

            let frame_left_w = self
                .decoration_theme
                .background_texture(DecorBackgroundName::Left, bg_state)
                .size()
                .to_logical(1, Transform::Normal)
                .w;
            let frame_right_w = self
                .decoration_theme
                .background_texture(DecorBackgroundName::Right, bg_state)
                .size()
                .to_logical(1, Transform::Normal)
                .w;
            let frame_top_h = match self.decoration_theme.title_background_textures(bg_state) {
                DecorTitleTextures::TitleStretched(texture) => texture.size().to_logical(1, Transform::Normal).h,
                DecorTitleTextures::Title5Part { title3, .. } => title3.size().to_logical(1, Transform::Normal).h,
            };
            let frame_bottom_h = self
                .decoration_theme
                .background_texture(DecorBackgroundName::Bottom, bg_state)
                .size()
                .to_logical(1, Transform::Normal)
                .h;

            let corner_top_left_size = self
                .decoration_theme
                .background_texture(DecorBackgroundName::TopLeft, bg_state)
                .size()
                .to_logical(1, Transform::Normal);
            let corner_top_right_size = self
                .decoration_theme
                .background_texture(DecorBackgroundName::TopRight, bg_state)
                .size()
                .to_logical(1, Transform::Normal);
            let corner_bottom_left_size = self
                .decoration_theme
                .background_texture(DecorBackgroundName::BottomLeft, bg_state)
                .size()
                .to_logical(1, Transform::Normal);
            let corner_bottom_right_size = self
                .decoration_theme
                .background_texture(DecorBackgroundName::BottomRight, bg_state)
                .size()
                .to_logical(1, Transform::Normal);

            let total_frame_size = Size::<_, Logical>::new(
                frame_left_w + self.window_size.w + frame_right_w,
                frame_top_h
                    + frame_bottom_h
                    + if self.button_toggled_states.contains(ButtonToggledStates::Shade) {
                        0
                    } else {
                        self.window_size.h
                    },
            );

            let frame_top_size =
                Size::<_, Logical>::new(total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w, frame_top_h);

            if self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize() {
                self.left.extents = Rectangle::zero();
                self.right.extents = Rectangle::zero();
                self.bottom.extents = Rectangle::zero();
                // FIXME: we don't rembe the titlebar, but it does seem we may cut off a bit from
                // the top: figure this out.
                self.top_left.extents = Rectangle::zero();
                self.top_right.extents = Rectangle::zero();
                self.bottom_left.extents = Rectangle::zero();
                self.bottom_right.extents = Rectangle::zero();
            } else {
                self.top_left.extents = Rectangle::new((0, 0).into(), corner_top_left_size);
                self.top_right.extents = Rectangle::new((total_frame_size.w - corner_top_right_size.w, 0).into(), corner_top_right_size);

                // FIXME: if shaded, position bottom bits right below the title's extents
                self.bottom_left.extents =
                    Rectangle::new((0, total_frame_size.h - corner_bottom_left_size.h).into(), corner_bottom_left_size);
                self.bottom_right.extents =
                    Rectangle::new((total_frame_size - corner_bottom_right_size).to_point(), corner_bottom_right_size);
                self.bottom.extents = Rectangle::new(
                    (corner_bottom_left_size.w, total_frame_size.h - frame_bottom_h).into(),
                    (
                        total_frame_size.w - corner_bottom_left_size.w - corner_bottom_right_size.w,
                        frame_bottom_h,
                    )
                        .into(),
                );

                if self.button_toggled_states.contains(ButtonToggledStates::Shade) {
                    self.left.extents = Rectangle::zero();
                    self.right.extents = Rectangle::zero();
                } else {
                    self.left.extents = Rectangle::new(
                        (0, frame_top_h).into(),
                        (frame_left_w, self.window_size.h + frame_bottom_h - corner_bottom_left_size.h).into(),
                    );
                    self.right.extents = Rectangle::new(
                        (total_frame_size.w - frame_right_w, frame_top_h).into(),
                        (frame_right_w, self.window_size.h + frame_bottom_h - corner_bottom_right_size.h).into(),
                    );
                }
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
                let btn_state = DecorButtonState::from((*btn, bg_state, self.button_hover_state, self.button_pressed_state));
                let btn_tex = self.decoration_theme.button_texture(btn_name, btn_state);
                let btn_size = btn_tex.size().to_logical(1, Transform::Normal);

                if btn_x + btn_size.w + btn_spacing < btn_right {
                    let extents = Rectangle::new((btn_x, (frame_top_h - btn_size.h + 1) / 2).into(), btn_size);
                    tracing::debug!("putting btn {btn:?} in left at ({}, {})", extents.loc.x, extents.loc.y);
                    btn_x += btn_size.w + btn_spacing;
                    *self.extents_for_button_mut(*btn) = extents;
                    visible_buttons.insert(*btn);
                }
            }

            let btn_left = btn_x + btn_spacing;
            let mut btn_x = total_frame_size.w - frame_right_w + btn_spacing - btn_offset;

            for btn in button_layout.end.iter().rev() {
                let btn_name = DecorButtonName::from((*btn, self.button_toggled_states));
                let btn_state = DecorButtonState::from((*btn, bg_state, self.button_hover_state, self.button_pressed_state));
                let btn_tex = self.decoration_theme.button_texture(btn_name, btn_state);
                let btn_size = btn_tex.size().to_logical(1, Transform::Normal);

                if btn_x - btn_size.w - btn_spacing > btn_left {
                    btn_x -= btn_size.w + btn_spacing;
                    let extents = Rectangle::new((btn_x, (frame_top_h - btn_size.h + 1) / 2).into(), btn_size);
                    tracing::debug!("putting btn {btn:?} in right at ({}, {})", extents.loc.x, extents.loc.y);
                    *self.extents_for_button_mut(*btn) = extents;
                    visible_buttons.insert(*btn);
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
                    *self.extents_for_button_mut(btn) = Rectangle::zero();
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

                let ctx = self.font_map.create_context();
                pangocairo::context_set_font_options(&ctx, Some(&self.font_options));

                let layout = pango::Layout::new(&ctx);
                layout.set_text(self.window_title.as_deref().unwrap_or(""));
                layout.set_font_description(Some(&pango::FontDescription::from_string(&self.config.title_font())));
                layout.set_auto_dir(false);
                let attr_list = {
                    let list = pango::AttrList::new();
                    list.insert(pango::AttrFloat::new_scale(self.scale));
                    list
                };
                layout.set_attributes(Some(&attr_list));
                let (_, title_extents) = layout.extents();
                tracing::debug!("on creation, title extents: {}x{}", title_extents.width(), title_extents.height());
                let title_extents = Rectangle::<_, Physical>::new(
                    (
                        pango::units_to_double(title_extents.x()).round() as i32,
                        pango::units_to_double(title_extents.y()).round() as i32,
                    )
                        .into(),
                    (
                        pango::units_to_double(title_extents.width()).round() as i32,
                        pango::units_to_double(title_extents.height()).round() as i32,
                    )
                        .into(),
                );

                let title_height = title_extents.size.h;
                let mut title_y = voffset + (frame_top_h - title_height) / 2;
                if title_y + title_height > frame_top_h {
                    title_y = 0.max(frame_top_h - title_height);
                }

                let title_bg_textures = self.decoration_theme.title_background_textures(bg_state);
                let top_height = if let DecorTitleTextures::Title5Part { top3: Some(top3), .. } = &title_bg_textures {
                    top3.size().to_logical(1, Transform::Normal).h
                } else if frame_border_top > 0 {
                    frame_border_top
                } else {
                    (frame_top_h / 10 + 1).min(title_y - 1).max(0)
                };

                let w1;
                let (w2, w4) = if let DecorTitleTextures::Title5Part { title2, title4, .. } = &title_bg_textures {
                    (
                        title2.size().to_logical(1, Transform::Normal).w,
                        title4.size().to_logical(1, Transform::Normal).w,
                    )
                } else {
                    (0, 0)
                };
                let w3;
                let w5;

                if self.config.full_width_title() {
                    w1 = btn_left;
                    w5 = frame_top_size.w - btn_right;
                    w3 = (frame_top_size.w - w1 - w2 - w4 - w5).max(0);

                    hoffset = match self.config.title_alignment() {
                        TitleAlignment::Left => self.config.title_horizontal_offset(),
                        TitleAlignment::Right => w3 - title_extents.size.w - self.config.title_horizontal_offset(),
                        TitleAlignment::Center => (w3 / 2) - (title_extents.size.w / 2),
                    }
                    .max(self.config.title_horizontal_offset());
                } else {
                    let title_shadow = if bg_state == DecorBackgroundState::Active {
                        self.config.title_shadow_active()
                    } else {
                        self.config.title_shadow_inactive()
                    } as i32; // FIXME: this seems wrong
                    w3 = (title_extents.size.w + title_shadow).min(frame_top_size.w - w2 - w4).max(0);
                    w5 = frame_top_size.w;

                    w1 = match self.config.title_alignment() {
                        TitleAlignment::Left => btn_left + self.config.title_horizontal_offset(),
                        TitleAlignment::Right => btn_right - w2 - w3 - w4 - self.config.title_horizontal_offset(),
                        TitleAlignment::Center => btn_left + ((btn_right - btn_left) / 2) - (w3 / 2) - w2,
                    }
                    .max(btn_left);
                }

                match &title_bg_textures {
                    DecorTitleTextures::TitleStretched(_) => {
                        if let TitleTextureData::Title5Part { .. } = &self.title {
                            self.title = TitleTextureData::new_stretched();
                        }
                    }
                    DecorTitleTextures::Title5Part { .. } => {
                        if let TitleTextureData::TitleStretched(_) = &self.title {
                            self.title = TitleTextureData::new_5part();
                        }
                    }
                }

                fn draw_title_text(
                    layout: pango::Layout,
                    extents: Rectangle<i32, Physical>,
                    config: &Xfwl4Config,
                    state: DecorBackgroundState,
                ) -> anyhow::Result<MemoryRenderBuffer> {
                    tracing::debug!(
                        "rendering window title text, extents={}x{}+{}+{}",
                        extents.size.w,
                        extents.size.h,
                        extents.loc.x,
                        extents.loc.y
                    );
                    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, extents.size.w, extents.size.h)?;
                    let cr = cairo::Context::new(&surface)?;

                    cr.translate(extents.loc.x as f64, extents.loc.y as f64);

                    let title_shadow = if state == DecorBackgroundState::Active {
                        config.title_shadow_active()
                    } else {
                        config.title_shadow_inactive()
                    };

                    if title_shadow != TitleShadow::None {
                        let title_shadow_color = if state == DecorBackgroundState::Active {
                            config.active_text_shadow_color()
                        } else {
                            config.inactive_text_shadow_color()
                        };

                        if let Some(rgba) = title_shadow_color {
                            GdkContextExt::set_source_rgba(&cr, &rgba);

                            if title_shadow == TitleShadow::Under {
                                cr.translate(1., 1.);
                                pangocairo::functions::show_layout(&cr, &layout);
                                cr.translate(-1., -1.);
                            } else {
                                cr.translate(-1., 0.);
                                pangocairo::functions::show_layout(&cr, &layout);
                                cr.translate(1., -1.);
                                pangocairo::functions::show_layout(&cr, &layout);
                                cr.translate(1., 1.);
                                pangocairo::functions::show_layout(&cr, &layout);
                                cr.translate(-1., 1.);
                                pangocairo::functions::show_layout(&cr, &layout);
                                cr.translate(0., -1.);
                            }
                        }
                    }

                    let title_color = if state == DecorBackgroundState::Active {
                        config.active_text_color()
                    } else {
                        config.inactive_text_color()
                    };

                    if let Some(rgba) = title_color {
                        GdkContextExt::set_source_rgba(&cr, &rgba);
                        tracing::debug!("drawing title text with color {rgba:?}");
                        pangocairo::functions::show_layout(&cr, &layout);
                    }

                    // surface.data() needs exclusive access to 'surface', but 'cr' will still hold
                    // onto it without an explicit drop.
                    drop(cr);

                    let w = surface.width();
                    let h = surface.height();
                    Ok(MemoryRenderBuffer::from_slice(
                        &surface.data()?,
                        Fourcc::Argb8888,
                        Size::new(w, h),
                        1,
                        Transform::Normal,
                        None,
                    ))
                }

                let title_x;
                match (&title_bg_textures, &mut self.title) {
                    (DecorTitleTextures::TitleStretched(texture), TitleTextureData::TitleStretched(texture_data)) => {
                        // FIXME: xfwm4 draws into both top_pm and title_pm, with different
                        // extents
                        let texture_size = texture.size().to_logical(1, Transform::Normal);
                        texture_data.extents =
                            Rectangle::new((corner_top_left_size.w + x, 0).into(), (frame_top_size.w, texture_size.h).into());

                        title_x = hoffset + w1 + w2;
                        self.title_text.extents = match draw_title_text(layout, title_extents, &self.config, bg_state) {
                            Ok(title_buffer) => {
                                self.title_buffer = title_buffer;
                                Rectangle::new(
                                    (corner_top_left_size.w + title_x, title_y).into(),
                                    (btn_right - w4, frame_top_h).into(),
                                )
                            }
                            Err(err) => {
                                warn!("Failed to render title text: {err}");
                                Rectangle::zero()
                            }
                        };
                    }

                    (
                        DecorTitleTextures::Title5Part { title3, .. },
                        TitleTextureData::Title5Part {
                            title1: title1_data,
                            top1: top1_data,
                            title2: title2_data,
                            top2: top2_data,
                            title3: title3_data,
                            top3: top3_data,
                            title4: title4_data,
                            top4: top4_data,
                            title5: title5_data,
                            top5: top5_data,
                        },
                    ) => {
                        let title3_size = title3.size().to_logical(1, Transform::Normal);

                        if w1 > 0 {
                            title1_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, title3_size.h).into());
                            top1_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, top_height).into());
                            x += w1;
                        } else {
                            title1_data.extents = Rectangle::zero();
                            top1_data.extents = Rectangle::zero();
                        }

                        title2_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, title3_size.h).into());
                        top2_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, top_height).into());
                        x += w2;

                        self.title_text.extents = if w3 > 0 {
                            title3_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, title3_size.h).into());
                            top3_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, top_height).into());
                            title_x = hoffset + x;
                            x += w3;

                            match draw_title_text(layout, title_extents, &self.config, bg_state) {
                                Ok(title_buffer) => {
                                    self.title_buffer = title_buffer;
                                    Rectangle::new(
                                        (corner_top_left_size.w + title_x, title_y).into(),
                                        (btn_right - w4, frame_top_h).into(),
                                    )
                                }
                                Err(err) => {
                                    warn!("Failed to render title text: {err}");
                                    Rectangle::zero()
                                }
                            }
                        } else {
                            title3_data.extents = Rectangle::zero();
                            top3_data.extents = Rectangle::zero();
                            Rectangle::zero()
                        };

                        x = x.min(btn_right - w4);
                        title4_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, title3_size.h).into());
                        top4_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, top_height).into());
                        x += w4;

                        if w5 > 0 {
                            title5_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5, title3_size.h).into());
                            top5_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5, top_height).into());
                        } else {
                            title5_data.extents = Rectangle::zero();
                            top5_data.extents = Rectangle::zero();
                        }
                    }

                    _ => unreachable!(),
                }
            }

            // TODO: input shape?
        }
    }

    #[inline]
    fn extents_for_button_mut(&mut self, btn: TitlebarButton) -> &mut Rectangle<i32, Logical> {
        match btn {
            TitlebarButton::Menu => &mut self.menu.extents,
            TitlebarButton::Hide => &mut self.hide.extents,
            TitlebarButton::Stick => &mut self.stick.extents,
            TitlebarButton::Shade => &mut self.shade.extents,
            TitlebarButton::Close => &mut self.close.extents,
            TitlebarButton::Maximize => &mut self.maximize.extents,
            TitlebarButton::SideSeparator => unreachable!(),
        }
    }

    fn button_state_for(&self, btn: TitlebarButton, bg_state: DecorBackgroundState) -> DecorButtonState {
        (btn, bg_state, self.button_hover_state, self.button_pressed_state).into()
    }
}

render_elements! {
    pub DecorationRenderElement<=GlesRenderer>;
    Texture=TextureRenderElement<GlesTexture>,
    TiledTexture=TextureShaderElement,
    RenderBuffer=MemoryRenderBufferRenderElement<GlesRenderer>,
}

impl std::fmt::Debug for DecorationRenderElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Texture(arg0) => f.debug_tuple("Texture").field(arg0).finish(),
            Self::TiledTexture(arg0) => f.debug_tuple("TiledTexture").field(arg0).finish(),
            Self::RenderBuffer(arg0) => f.debug_tuple("RenderBuffer").field(arg0).finish(),
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
        let location = location.to_f64();
        let buffer_scale = 1;

        let bg_state = if self.is_active {
            DecorBackgroundState::Active
        } else {
            DecorBackgroundState::Inactive
        };

        let tiling_shader = self.decoration_theme.tiling_shader();

        let title_text_elem = if !self.title_text.extents.is_empty() {
            let title_location = location + self.title_text.extents.loc.to_f64().to_physical(scale);
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                title_location,
                &self.title_buffer,
                Some(alpha),
                None,
                None,
                Kind::Unspecified,
            )
            .ok()
            .map(DecorationRenderElement::RenderBuffer)
            .into_iter()
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let title_elems = match (self.decoration_theme.title_background_textures(bg_state), &self.title) {
            (DecorTitleTextures::TitleStretched(texture), TitleTextureData::TitleStretched(texture_data)) => {
                create_render_elem(renderer, tiling_shader, texture, texture_data, location, buffer_scale, scale, alpha)
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
                TitleTextureData::Title5Part {
                    title1: title1_data,
                    top1: top1_data,
                    title2: title2_data,
                    top2: top2_data,
                    title3: title3_data,
                    top3: top3_data,
                    title4: title4_data,
                    top4: top4_data,
                    title5: title5_data,
                    top5: top5_data,
                },
            ) => [
                (top1, top1_data),
                (top2, top2_data),
                (top3, top3_data),
                (top4, top4_data),
                (top5, top5_data),
                (Some(title1), title1_data),
                (Some(title2), title2_data),
                (Some(title3), title3_data),
                (Some(title4), title4_data),
                (Some(title5), title5_data),
            ]
            .into_iter()
            .flat_map(|(maybe_texture, texture_data)| {
                if let Some(texture) = maybe_texture {
                    create_render_elem(renderer, tiling_shader, texture, texture_data, location, buffer_scale, scale, alpha)
                } else {
                    vec![]
                }
            })
            .collect(),

            _ => unreachable!(),
        };

        [
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme
                    .button_texture(DecorButtonName::Hide, self.button_state_for(TitlebarButton::Hide, bg_state)),
                &self.hide,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme
                    .button_texture(DecorButtonName::Menu, self.button_state_for(TitlebarButton::Menu, bg_state)),
                &self.menu,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme
                    .button_texture(DecorButtonName::Close, self.button_state_for(TitlebarButton::Close, bg_state)),
                &self.close,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            {
                let btn_name = (TitlebarButton::Maximize, self.button_toggled_states).into();
                create_render_elem(
                    renderer,
                    tiling_shader,
                    self.decoration_theme
                        .button_texture(btn_name, self.button_state_for(TitlebarButton::Maximize, bg_state)),
                    &self.maximize,
                    location,
                    buffer_scale,
                    scale,
                    alpha,
                )
            },
            {
                let btn_name = (TitlebarButton::Stick, self.button_toggled_states).into();
                create_render_elem(
                    renderer,
                    tiling_shader,
                    self.decoration_theme
                        .button_texture(btn_name, self.button_state_for(TitlebarButton::Stick, bg_state)),
                    &self.stick,
                    location,
                    buffer_scale,
                    scale,
                    alpha,
                )
            },
            {
                let btn_name = (TitlebarButton::Shade, self.button_toggled_states).into();
                create_render_elem(
                    renderer,
                    tiling_shader,
                    self.decoration_theme
                        .button_texture(btn_name, self.button_state_for(TitlebarButton::Shade, bg_state)),
                    &self.shade,
                    location,
                    buffer_scale,
                    scale,
                    alpha,
                )
            },
            title_text_elem,
            title_elems,
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Left, bg_state),
                &self.left,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Right, bg_state),
                &self.right,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Bottom, bg_state),
                &self.bottom,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::BottomLeft, bg_state),
                &self.bottom_left,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::BottomRight, bg_state),
                &self.bottom_right,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::TopLeft, bg_state),
                &self.top_left,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
            create_render_elem(
                renderer,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::TopRight, bg_state),
                &self.top_right,
                location,
                buffer_scale,
                scale,
                alpha,
            ),
        ]
        .into_iter()
        .flatten()
        .map(C::from)
        .collect()
    }
}

impl WindowElement {
    pub fn decoration_state(&self) -> RefMut<'_, WindowState> {
        self.user_data()
            .insert_if_missing(|| RefCell::new(WindowState { window_decorations: None }));

        self.user_data().get::<RefCell<WindowState>>().unwrap().borrow_mut()
    }

    pub fn enable_decorations(
        &self,
        window_size: Size<i32, Logical>,
        scale: f64,
        config: &Xfwl4Config,
        decoration_theme: &DecorationTheme,
        font_map: &pango::FontMap,
        font_options: &cairo::FontOptions,
    ) {
        let mut decoration_state = self.decoration_state();
        if decoration_state.window_decorations.is_none() {
            let window_title = match self.0.underlying_surface() {
                WindowSurface::Wayland(toplevel_surface) => compositor::with_states(toplevel_surface.wl_surface(), |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().unwrap().title.clone())
                }),
                WindowSurface::X11(x11_surface) => Some(x11_surface.title()),
            };

            decoration_state.window_decorations = Some(WindowDecorations::new(
                window_size,
                window_title,
                scale,
                config.clone(),
                decoration_theme.clone(),
                font_map.clone(),
                font_options.clone(),
            ));
        }
    }

    pub fn disable_decorations(&self) {
        self.decoration_state().window_decorations = None;
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn enable_decorations_for_window(&self, window: &WindowElement) {
        let window_size = SpaceElement::geometry(&window.0).size;
        let scale = self
            .workspace_manager
            .outputs_for_element(window)
            .first()
            .or_else(|| self.workspace_manager.outputs().next())
            .map(|output| output.current_scale().fractional_scale())
            .unwrap_or(1.0);
        window.enable_decorations(
            window_size,
            scale,
            &self.config,
            self.decoration_theme.as_ref().unwrap(),
            &self.font_map,
            &self.font_options,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn create_render_elem(
    renderer: &GlesRenderer,
    tiling_shader: &GlesTexProgram,
    texture: &DecorTexture,
    texture_data: &TextureData,
    location_offset: Point<f64, Physical>,
    buffer_scale: i32,
    scale: Scale<f64>,
    alpha: f32,
) -> Vec<DecorationRenderElement> {
    if texture_data.extents.is_empty() {
        vec![]
    } else {
        let location = location_offset + texture_data.extents.loc.to_f64().to_physical(scale);
        vec![match texture.rendering_mode() {
            DecorRenderingMode::Tiled(direction) => DecorationRenderElement::TiledTexture(create_tiled_texture_elem(
                renderer,
                texture_data.id.clone(),
                texture,
                tiling_shader,
                location,
                texture_data.extents.size,
                buffer_scale,
                scale,
                alpha,
                direction,
            )),
            DecorRenderingMode::Stretched(_) => DecorationRenderElement::Texture(create_texture_elem(
                renderer,
                texture_data.id.clone(),
                texture,
                location,
                texture_data.extents.size,
                buffer_scale,
                alpha,
            )),
            DecorRenderingMode::AsIs => DecorationRenderElement::Texture(create_texture_elem(
                renderer,
                texture_data.id.clone(),
                texture,
                location,
                texture_data.extents.size,
                buffer_scale,
                alpha,
            )),
        }]
    }
}

fn create_texture_elem(
    renderer: &GlesRenderer,
    id: Id,
    texture: &GlesTexture,
    location: Point<f64, Physical>,
    render_size: Size<i32, Logical>,
    buffer_scale: i32,
    alpha: f32,
) -> TextureRenderElement<GlesTexture> {
    let tex_size = texture.size();
    let src = Rectangle::new((0, 0).into(), tex_size)
        .to_logical(buffer_scale, Transform::Normal, &tex_size)
        .to_f64();
    TextureRenderElement::from_static_texture(
        id,
        renderer.context_id(),
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
    renderer: &GlesRenderer,
    id: Id,
    texture: &GlesTexture,
    shader: &GlesTexProgram,
    location: Point<f64, Physical>,
    render_size: Size<i32, Logical>,
    buffer_scale: i32,
    scale: Scale<f64>,
    alpha: f32,
    direction: Direction,
) -> TextureShaderElement {
    let element = create_texture_elem(renderer, id, texture, location, render_size, buffer_scale, alpha);

    let tex_size = texture.size().to_f64();
    let geo_size = render_size.to_f64().to_physical(scale);

    let tile_mask = match direction {
        Direction::Horizontal => (1.0f32, 0.0f32),
        Direction::Vertical => (0.0f32, 1.0f32),
    };

    let uniforms = [
        Uniform::new("tex_size", UniformValue::_2f(tex_size.w as f32, tex_size.h as f32)),
        Uniform::new("geo_size", UniformValue::_2f(geo_size.w as f32, geo_size.h as f32)),
        Uniform::new("tile_mask", UniformValue::_2f(tile_mask.0, tile_mask.1)),
    ]
    .to_vec();

    TextureShaderElement::new(element, shader.clone(), uniforms)
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

impl From<(TitlebarButton, DecorBackgroundState, ButtonHoverState, ButtonPressedState)> for DecorButtonState {
    fn from((tbtn, bg_state, hover, pressed): (TitlebarButton, DecorBackgroundState, ButtonHoverState, ButtonPressedState)) -> Self {
        let (hover_state, pressed_state) = match tbtn {
            TitlebarButton::Menu => (ButtonHoverState::Menu, ButtonPressedState::Menu),
            TitlebarButton::Hide => (ButtonHoverState::Hide, ButtonPressedState::Hide),
            TitlebarButton::Close => (ButtonHoverState::Close, ButtonPressedState::Close),
            TitlebarButton::Maximize => (ButtonHoverState::Maximize, ButtonPressedState::Maximize),
            TitlebarButton::Stick => (ButtonHoverState::Stick, ButtonPressedState::Stick),
            TitlebarButton::Shade => (ButtonHoverState::Shade, ButtonPressedState::Shade),
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

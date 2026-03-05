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
use gtk::{
    cairo,
    gdk::prelude::GdkContextExt,
    gdk_pixbuf,
    pango::{self, traits::FontMapExt},
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ContextId, Frame, ImportMem, Offscreen, Renderer, Texture,
            element::{AsRenderElements, Id, Kind, texture::TextureRenderElement},
            gles::{GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformValue, element::TextureShaderElement},
        },
    },
    desktop::{WindowSurface, space::SpaceElement},
    input::Seat,
    output::Scale as OutputScale,
    render_elements,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform},
};

use std::{
    cell::{RefCell, RefMut},
    collections::HashSet,
};

use crate::{
    backend::Backend,
    core::{
        config::{TitleAlignment, TitleShadow, TitlebarButton, Xfwl4Config},
        cursor::CursorName,
        drawing::decorations::{
            DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode, DecorTexture,
            DecorTitleTextures, DecorationTheme, Direction,
        },
        shell::{
            GrabTrigger, ResizeEdge,
            xdg::{desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel, window_title_for_xdg_toplevel},
        },
        state::Xfwl4State,
        util::{
            ImageData,
            icon_theme::{FreedesktopIconsIconTheme, IconTheme},
        },
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

    fn point_in(&self, loc: Point<f64, Logical>) -> bool {
        if self.extents.is_empty() {
            false
        } else {
            self.extents.contains(loc.to_i32_ceil())
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
enum HoverState {
    None,
    Close,
    Hide,
    Maximize,
    Menu,
    Shade,
    Stick,
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
enum PressedState {
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
    struct ButtonToggledStates: u8 {
        const Maximize = (1 << 0);
        const Shade = (1 << 1);
        const Stick = (1 << 2);
    }
}

struct TitlebarCache {
    texture: RefCell<Option<GlesTexture>>,
    extents: Rectangle<i32, Logical>,
}

impl std::fmt::Debug for TitlebarCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TitlebarCache")
            .field("extents", &self.extents)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct PixelBuffer {
    data: Vec<u8>,
    size: Size<i32, Buffer>,
    format: Fourcc,
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

    top_clip: i32,

    is_active: bool,
    hover_state: HoverState,
    pressed_state: PressedState,
    button_toggled_states: ButtonToggledStates,

    window_icon: Option<ImageData>,
    window_icon_pixels: Option<PixelBuffer>,
    window_icon_data: TextureData,

    titlebar_cache: TitlebarCache,
    title_text_pixels: Option<PixelBuffer>,
    title_text: TextureData,

    top_left: TextureData,
    top: TextureData, // pseudo-side just for resize extents
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
    #[allow(clippy::too_many_arguments)]
    fn new(
        window_size: Size<i32, Logical>,
        window_title: Option<String>,
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
            top_clip: 0,
            is_active: false,
            hover_state: HoverState::None,
            pressed_state: PressedState::None,
            button_toggled_states: ButtonToggledStates::empty(),
            window_icon,
            window_icon_pixels: None,
            window_icon_data: TextureData::new(),
            titlebar_cache: TitlebarCache {
                texture: RefCell::new(None),
                extents: Rectangle::zero(),
            },
            title_text_pixels: None,
            title_text: TextureData::new(),
            top_left: TextureData::new(),
            top: TextureData::new(),
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

    pub fn pointer_motion<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        _window: &WindowElement,
        _serial: Serial,
        loc: Point<f64, Logical>,
    ) {
        self.pointer_loc = Some(loc);

        let mut buttons = [
            (&mut self.close, HoverState::Close),
            (&mut self.hide, HoverState::Hide),
            (&mut self.maximize, HoverState::Maximize),
            (&mut self.menu, HoverState::Menu),
            (&mut self.shade, HoverState::Shade),
            (&mut self.stick, HoverState::Stick),
        ];

        let new_hover_state = buttons
            .iter_mut()
            .find_map(|(data, flag)| data.point_in(loc).then_some(*flag))
            .unwrap_or(HoverState::None);

        if new_hover_state != HoverState::None {
            if new_hover_state != self.hover_state {
                self.hover_state = new_hover_state;
                self.titlebar_cache.texture.replace(None);
            }
            state.core.set_cursor(CursorName::Default);
        } else {
            let resize_grips = [
                (&self.top_left, HoverState::TopLeft, CursorName::TopLeftCorner),
                (&self.top, HoverState::Top, CursorName::TopSide),
                (&self.top_right, HoverState::TopRight, CursorName::TopRightCorner),
                (&self.left, HoverState::Left, CursorName::LeftSide),
                (&self.right, HoverState::Right, CursorName::RightSide),
                (&self.bottom_left, HoverState::BottomLeft, CursorName::BottomLeftCorner),
                (&self.bottom, HoverState::Bottom, CursorName::BottomSide),
                (&self.bottom_right, HoverState::BottomRight, CursorName::BottomRightCorner),
            ];

            let (new_hover_state, new_cursor_name) = resize_grips
                .iter()
                .find_map(|(data, flag, cursor)| data.point_in(loc).then_some((*flag, *cursor)))
                .unwrap_or((HoverState::None, CursorName::Default));

            if new_hover_state != self.hover_state {
                state.core.set_cursor(new_cursor_name);
                self.hover_state = new_hover_state;
                self.titlebar_cache.texture.replace(None);
            }
        }
    }

    pub fn pointer_leave<BackendData: Backend>(&mut self, state: &mut Xfwl4State<BackendData>) {
        self.pointer_loc = None;

        let needs_rerender = is_button_hover(self.hover_state) || is_button_pressed(self.pressed_state);

        match self.hover_state {
            HoverState::None => (),
            _ if !is_button_hover(self.hover_state) => state.core.set_cursor(CursorName::Default),
            _ => (),
        }
        self.hover_state = HoverState::None;
        self.pressed_state = PressedState::None;

        if needs_rerender {
            self.titlebar_cache.texture.replace(None);
        }
    }

    fn button_press_or_touch_down<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        serial: Serial,
        trigger: GrabTrigger,
    ) {
        if let Some(pointer_loc) = self.pointer_loc.as_ref() {
            let mut buttons = [
                (&mut self.close, PressedState::Close),
                (&mut self.hide, PressedState::Hide),
                (&mut self.maximize, PressedState::Maximize),
                (&mut self.menu, PressedState::Menu),
                (&mut self.shade, PressedState::Shade),
                (&mut self.stick, PressedState::Stick),
            ];

            let new_pressed_state = buttons
                .iter_mut()
                .find_map(|(data, flag)| data.point_in(*pointer_loc).then_some(*flag))
                .unwrap_or(PressedState::None);

            if new_pressed_state == PressedState::Menu {
                let window = window.clone();
                let seat = seat.clone();
                let location = pointer_loc.to_i32_round() - self.decorations_offset();
                state.core.handle.insert_idle(move |state| {
                    state.pop_up_window_menu(&window, &seat, serial, location);
                });
                // XXX: not bothering with a persistent pressed state for the menu button; I'm not
                // sure this is actually the right thing to do.
                self.pressed_state = PressedState::None;
            } else if new_pressed_state != PressedState::None {
                if new_pressed_state != self.pressed_state {
                    self.pressed_state = new_pressed_state;
                    self.titlebar_cache.texture.replace(None);
                }
            } else {
                let titlebar_parts = match &self.title {
                    TitleTextureData::TitleStretched(data) => vec![data],
                    TitleTextureData::Title5Part {
                        title1,
                        title2,
                        title3,
                        title4,
                        title5,
                        ..
                    } => {
                        vec![title1, title2, title3, title4, title5]
                    }
                }
                .into_iter()
                .map(|part| (part, PressedState::Titlebar));

                let resize_grips = [
                    (&self.top_left, PressedState::TopLeft),
                    (&self.top, PressedState::Top),
                    (&self.top_right, PressedState::TopRight),
                    (&self.left, PressedState::Left),
                    (&self.right, PressedState::Right),
                    (&self.bottom_left, PressedState::BottomLeft),
                    (&self.bottom, PressedState::Bottom),
                    (&self.bottom_right, PressedState::BottomRight),
                ];

                let mut move_resize_grips = resize_grips.into_iter().chain(titlebar_parts);

                let new_pressed_state = move_resize_grips
                    .find_map(|(data, flag)| data.point_in(*pointer_loc).then_some(flag))
                    .unwrap_or(PressedState::None);

                if new_pressed_state != self.pressed_state {
                    if new_pressed_state != PressedState::None {
                        let seat = seat.clone();
                        let window = window.clone();

                        if new_pressed_state == PressedState::Titlebar {
                            state
                                .core
                                .handle
                                .insert_idle(move |state| state.start_maybe_window_move(window, seat, serial, trigger));
                        } else if let Ok(edges) = ResizeEdge::try_from(new_pressed_state) {
                            state
                                .core
                                .handle
                                .insert_idle(move |state| state.start_maybe_window_resize(window, seat, serial, edges, trigger));
                        }
                    }

                    self.pressed_state = new_pressed_state;
                }
            }
        }
    }

    pub fn button_press<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        serial: Serial,
    ) {
        self.button_press_or_touch_down(seat, state, window, serial, GrabTrigger::Pointer);
    }

    pub fn touch_down<BackendData: Backend>(
        &mut self,
        seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        serial: Serial,
    ) {
        self.button_press_or_touch_down(seat, state, window, serial, GrabTrigger::Touch);
    }

    pub fn button_release<BackendData: Backend>(
        &mut self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        state: &mut Xfwl4State<BackendData>,
        window: &WindowElement,
        _serial: Serial,
    ) {
        if self.pressed_state != PressedState::None {
            if let Some(pointer_loc) = self.pointer_loc.as_ref() {
                let buttons = [
                    (&self.close, PressedState::Close),
                    (&self.hide, PressedState::Hide),
                    (&self.maximize, PressedState::Maximize),
                    (&self.menu, PressedState::Menu),
                    (&self.shade, PressedState::Shade),
                    (&self.stick, PressedState::Stick),
                ];

                let final_pressed_state = buttons.iter().find_map(|(data, flag)| data.point_in(*pointer_loc).then_some(*flag));
                let final_pressed_state = final_pressed_state.unwrap_or(PressedState::None);

                if final_pressed_state == self.pressed_state {
                    match final_pressed_state {
                        PressedState::None => (),
                        PressedState::Hide => {
                            state.set_window_minimized(window);
                        }
                        PressedState::Menu => (), // We pop up the menu on press
                        PressedState::Close => window.close(),
                        PressedState::Shade => {
                            state.set_window_shaded(window, !self.button_toggled_states.contains(ButtonToggledStates::Shade));
                        }
                        PressedState::Stick => (), // TODO
                        PressedState::Maximize => {
                            // Use an idle function here because we otherwise end up recursively trying
                            // to borrow the RefCell that WindowDecorations (aka 'self') is in, and
                            // crash.
                            let window = window.clone();
                            let new_is_maximized = !self.button_toggled_states.contains(ButtonToggledStates::Maximize);
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_maximized(&window, new_is_maximized);
                            });
                        }
                        _ => (),
                    }
                }
            }

            if is_button_pressed(self.pressed_state) {
                self.pressed_state = PressedState::None;
                self.titlebar_cache.texture.replace(None);
            } else {
                self.pressed_state = PressedState::None;
            }
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
                self.window_icon_pixels = None;
            }
            self.decoration_theme = decoration_theme.clone();
            self.update();
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
            self.window_icon_pixels = None;
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

    pub fn update_app_icon(&mut self, window_icon: Option<ImageData>) {
        if self.config.show_app_icon() && self.config.button_layout().includes(TitlebarButton::Menu) {
            self.window_icon = window_icon;
            self.window_icon_pixels = None;
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

    pub fn update(&mut self) {
        profiling::scope!("WindowDecorations::update");
        if self.window_size.w > 0 && self.window_size.h > 0 {
            let bg_state = if self.is_active {
                DecorBackgroundState::Active
            } else {
                DecorBackgroundState::Inactive
            };

            let borderless_maximize =
                self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize();

            let frame_border_top = self.config.frame_border_top();
            let frame_top_h = match self.decoration_theme.title_background_textures(bg_state) {
                DecorTitleTextures::TitleStretched(texture) => texture.size().to_logical(1, Transform::Normal).h,
                DecorTitleTextures::Title5Part { title3, .. } => title3.size().to_logical(1, Transform::Normal).h,
            };
            let top_clip = if borderless_maximize {
                match self.decoration_theme.title_background_textures(bg_state) {
                    DecorTitleTextures::Title5Part { top3: Some(top3), .. } => top3.size().to_logical(1, Transform::Normal).h,
                    _ => frame_border_top,
                }
            } else {
                0
            };
            self.top_clip = top_clip;
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
                (
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::Left, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal)
                        .w,
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::Right, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal)
                        .w,
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::Bottom, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal)
                        .h,
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::TopLeft, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal),
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::TopRight, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal),
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::BottomLeft, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal),
                    self.decoration_theme
                        .background_texture(DecorBackgroundName::BottomRight, bg_state)
                        .size()
                        .to_logical(1, Transform::Normal),
                )
            };

            let total_frame_size = Size::<_, Logical>::new(
                frame_left_w + self.window_size.w + frame_right_w,
                visible_top_h
                    + frame_bottom_h
                    + if self.button_toggled_states.contains(ButtonToggledStates::Shade) {
                        0
                    } else {
                        self.window_size.h
                    },
            );

            let frame_top_size = Size::<_, Logical>::new(
                (total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w).max(0),
                frame_top_h,
            );

            self.top_left.extents = Rectangle::new((0, 0).into(), corner_top_left_size);
            self.top_right.extents = Rectangle::new((total_frame_size.w - corner_top_right_size.w, 0).into(), corner_top_right_size);
            self.top.extents = Rectangle::new(
                (corner_top_left_size.w, 0).into(),
                (
                    (total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w).max(0),
                    frame_bottom_h, // Make the top resize grip area the same height as the bottom
                )
                    .into(),
            );

            self.bottom_left.extents = Rectangle::new((0, total_frame_size.h - corner_bottom_left_size.h).into(), corner_bottom_left_size);
            self.bottom_right.extents = Rectangle::new((total_frame_size - corner_bottom_right_size).to_point(), corner_bottom_right_size);
            self.bottom.extents = Rectangle::new(
                (corner_bottom_left_size.w, total_frame_size.h - frame_bottom_h).into(),
                (
                    (total_frame_size.w - corner_bottom_left_size.w - corner_bottom_right_size.w).max(0),
                    frame_bottom_h,
                )
                    .into(),
            );

            if borderless_maximize || self.button_toggled_states.contains(ButtonToggledStates::Shade) {
                self.left.extents = Rectangle::zero();
                self.right.extents = Rectangle::zero();
            } else {
                self.left.extents = Rectangle::new(
                    (0, visible_top_h).into(),
                    (
                        frame_left_w,
                        (self.window_size.h + frame_bottom_h - corner_bottom_left_size.h).max(0),
                    )
                        .into(),
                );
                self.right.extents = Rectangle::new(
                    (total_frame_size.w - frame_right_w, visible_top_h).into(),
                    (
                        frame_right_w,
                        (self.window_size.h + frame_bottom_h - corner_bottom_right_size.h).max(0),
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
                    let btn_size = btn_tex.size().to_logical(1, Transform::Normal);

                    if btn_x + btn_size.w + btn_spacing < btn_right {
                        let extents = Rectangle::new((btn_x, (visible_top_h - btn_size.h + 1) / 2).into(), btn_size);
                        btn_x += btn_size.w + btn_spacing;
                        *self.extents_for_button_mut(*btn) = extents;
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
                    let btn_size = btn_tex.size().to_logical(1, Transform::Normal);

                    if btn_x - btn_size.w - btn_spacing > btn_left {
                        btn_x -= btn_size.w + btn_spacing;
                        let extents = Rectangle::new((btn_x, (visible_top_h - btn_size.h + 1) / 2).into(), btn_size);
                        *self.extents_for_button_mut(*btn) = extents;
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
                    *self.extents_for_button_mut(btn) = Rectangle::zero();
                }
            }

            if !self.menu.extents.is_empty() && self.window_icon_pixels.is_none() {
                profiling::scope!("load_window_icon");
                let pixbuf = self
                    .window_icon
                    .as_ref()
                    .and_then(|window_icon| {
                        window_icon.load(
                            self.menu.extents.size.w as u32,
                            self.menu.extents.size.h as u32,
                            1.0,
                            &self.icon_theme,
                        )
                    })
                    .or_else(|| {
                        self.icon_theme
                            .load_icon("xfwm4-default", self.menu.extents.size.w.min(self.menu.extents.size.h), 1.0)
                            .ok()
                    });
                if let Some(pixbuf) = pixbuf {
                    let icon_pixels = pixbuf_to_pixels(&pixbuf);
                    if let Some(pixels) = &icon_pixels {
                        let icon_size: Size<i32, Logical> = (pixels.size.w, pixels.size.h).into();
                        let menu = &self.menu.extents;
                        let xoff = (menu.size.w - icon_size.w) / 2;
                        let yoff = (menu.size.h - icon_size.h) / 2;
                        self.window_icon_data.extents = Rectangle::new((menu.loc.x + xoff, menu.loc.y + yoff).into(), icon_size);
                    } else {
                        self.window_icon_data.extents = Rectangle::zero();
                    }
                    self.window_icon_pixels = icon_pixels;
                } else {
                    self.window_icon_data.extents = Rectangle::zero();
                }
            } else if self.menu.extents.is_empty() {
                self.window_icon_pixels = None;
                self.window_icon_data.extents = Rectangle::zero();
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

                let (layout, title_extents) = {
                    profiling::scope!("pango_title_layout");
                    let ctx = self.font_map.create_context();
                    pangocairo::context_set_font_options(&ctx, Some(&self.font_options));

                    let layout = pango::Layout::new(&ctx);
                    layout.set_text(self.window_title.as_deref().unwrap_or(""));
                    layout.set_font_description(Some(&pango::FontDescription::from_string(&self.config.title_font())));
                    layout.set_auto_dir(false);
                    layout.set_attributes(Some(&{
                        let list = pango::AttrList::new();
                        list.insert(pango::AttrFloat::new_scale(self.scale.fractional_scale()));
                        list
                    }));
                    let (_, title_extents) = layout.extents();
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
                    (layout, title_extents)
                };

                let scale = self.scale.fractional_scale();
                let logical_title_size: Size<i32, Logical> = title_extents.size.to_f64().to_logical(scale).to_i32_round();
                let title_height = logical_title_size.h;
                let mut title_y = voffset + (visible_top_h - title_height) / 2;
                if title_y + title_height > visible_top_h {
                    title_y = 0.max(visible_top_h - title_height);
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

                if self.config.full_width_title() {
                    w1 = btn_left;
                    let w5 = frame_top_size.w - btn_right;
                    w3 = (frame_top_size.w - w1 - w2 - w4 - w5).max(0);

                    hoffset = match self.config.title_alignment() {
                        TitleAlignment::Left => self.config.title_horizontal_offset(),
                        TitleAlignment::Right => w3 - logical_title_size.w - self.config.title_horizontal_offset(),
                        TitleAlignment::Center => (w3 / 2) - (logical_title_size.w / 2),
                    }
                    .max(self.config.title_horizontal_offset());
                } else {
                    let title_shadow = if bg_state == DecorBackgroundState::Active {
                        self.config.title_shadow_active()
                    } else {
                        self.config.title_shadow_inactive()
                    } as i32; // FIXME: this seems wrong
                    w3 = (logical_title_size.w + title_shadow).min(frame_top_size.w - w2 - w4).max(0);

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

                let title_x;
                match (&title_bg_textures, &mut self.title) {
                    (DecorTitleTextures::TitleStretched(_), TitleTextureData::TitleStretched(texture_data)) => {
                        // FIXME: xfwm4 draws into both top_pm and title_pm, with different
                        // extents
                        texture_data.extents =
                            Rectangle::new((corner_top_left_size.w + x, 0).into(), (frame_top_size.w, visible_top_h).into());

                        title_x = hoffset + w1 + w2;
                        let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                        self.title_text_pixels =
                            render_title_text_pixels(layout, title_extents, title_max_width as f64 * scale, &self.config, bg_state);
                        self.title_text.extents = if self.title_text_pixels.is_some() {
                            Rectangle::new(
                                (corner_top_left_size.w + title_x, title_y).into(),
                                (btn_right - w4, visible_top_h).into(),
                            )
                        } else {
                            Rectangle::zero()
                        };
                    }

                    (
                        DecorTitleTextures::Title5Part { .. },
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
                        let visible_top_height = (top_height - top_clip).max(0);

                        if w1 > 0 {
                            title1_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, visible_top_h).into());
                            top1_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w1, visible_top_height).into());
                            x += w1;
                        } else {
                            title1_data.extents = Rectangle::zero();
                            top1_data.extents = Rectangle::zero();
                        }

                        title2_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, visible_top_h).into());
                        top2_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w2, visible_top_height).into());
                        x += w2;

                        self.title_text.extents = if w3 > 0 {
                            title3_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_h).into());
                            top3_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_height).into());
                            title_x = hoffset + x;
                            x += w3;

                            let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                            self.title_text_pixels =
                                render_title_text_pixels(layout, title_extents, title_max_width as f64 * scale, &self.config, bg_state);
                            if self.title_text_pixels.is_some() {
                                Rectangle::new(
                                    (corner_top_left_size.w + title_x, title_y).into(),
                                    (btn_right - w4, visible_top_h).into(),
                                )
                            } else {
                                Rectangle::zero()
                            }
                        } else {
                            title3_data.extents = Rectangle::zero();
                            top3_data.extents = Rectangle::zero();
                            Rectangle::zero()
                        };

                        x = x.min(btn_right - w4);
                        title4_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, visible_top_h).into());
                        top4_data.extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w4, visible_top_height).into());
                        x += w4;

                        // Compute the remaining width after all title parts, capped at the right
                        // edge of the frame top.  xfwm4 passes the full frame width to
                        // frameFillTitlePixmap() for title5 and relies on window clipping; we have
                        // to do the arithmetic explicitly.
                        let w5_remaining = (frame_top_size.w - x).max(0);
                        if w5_remaining > 0 {
                            title5_data.extents =
                                Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5_remaining, visible_top_h).into());
                            top5_data.extents =
                                Rectangle::new((corner_top_left_size.w + x, 0).into(), (w5_remaining, visible_top_height).into());
                        } else {
                            title5_data.extents = Rectangle::zero();
                            top5_data.extents = Rectangle::zero();
                        }
                    }

                    _ => unreachable!(),
                }
            }

            self.titlebar_cache.extents = Rectangle::new((0, 0).into(), (total_frame_size.w, visible_top_h).into());
            self.titlebar_cache.texture.replace(None);

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
        (btn, bg_state, self.hover_state, self.pressed_state).into()
    }

    fn composite_titlebar(
        &self,
        renderer: &mut GlesRenderer,
        bg_state: DecorBackgroundState,
        tiling_shader: &GlesTexProgram,
    ) -> anyhow::Result<Option<GlesTexture>> {
        profiling::scope!("WindowDecorations::composite_titlebar");

        let tb_size = self.titlebar_cache.extents.size;
        if tb_size.w > 0 && tb_size.h > 0 {
            let text_tex = self.title_text_pixels.as_ref().and_then(|p| {
                renderer
                    .import_memory(&p.data, p.format, p.size, false)
                    .inspect_err(|err| tracing::warn!("Failed to import title text texture: {err}"))
                    .ok()
            });
            let icon_tex = self.window_icon_pixels.as_ref().and_then(|p| {
                renderer
                    .import_memory(&p.data, p.format, p.size, false)
                    .inspect_err(|err| tracing::warn!("Failed to import window icon texture: {err}"))
                    .ok()
            });

            let src_offset = (self.top_clip > 0).then(|| Point::<i32, Buffer>::new(0, self.top_clip));

            // In order to get the text rendering correct (that is, rendered at the physical pixel
            // size that will actually be displayed on screen), we have to size the buffer scaled
            // by the output's fractional scale, and draw everything into it in the same way.  I'm
            // not sure, but this might cause some degradation of the quality of the theme images,
            // because they might end up being scaled up and then back down again.  If that's the
            // case, one option would be to render the title into its own standalone texture, and
            // have a render element just for the title.  I'd like to avoid that, of course, since
            // that means more pressure on smithay when rendering.
            let scale = self.scale.fractional_scale();
            let buffer_size = tb_size.to_f64().to_buffer(scale, Transform::Normal).to_i32_round();
            let physical_size = tb_size.to_f64().to_physical(scale).to_i32_round();

            let mut offscreen: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
            let mut fb = renderer.bind(&mut offscreen)?;
            let mut frame = renderer.render(&mut fb, physical_size, Transform::Normal)?;

            frame.clear([0., 0., 0., 0.].into(), &[Rectangle::from_size(physical_size)])?;

            draw_decor_texture(
                &mut frame,
                self.decoration_theme.background_texture(DecorBackgroundName::TopLeft, bg_state),
                &self.top_left.extents,
                None,
                scale,
                tiling_shader,
            )?;
            draw_decor_texture(
                &mut frame,
                self.decoration_theme.background_texture(DecorBackgroundName::TopRight, bg_state),
                &self.top_right.extents,
                None,
                scale,
                tiling_shader,
            )?;

            match (self.decoration_theme.title_background_textures(bg_state), &self.title) {
                (DecorTitleTextures::TitleStretched(texture), TitleTextureData::TitleStretched(data)) => {
                    draw_decor_texture(&mut frame, texture, &data.extents, src_offset, scale, tiling_shader)?;
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
                    for (tex, data) in [(title1, d1), (title2, d2), (title3, d3), (title4, d4), (title5, d5)] {
                        draw_decor_texture(&mut frame, tex, &data.extents, src_offset, scale, tiling_shader)?;
                    }
                    for (maybe_tex, data) in [(top1, dt1), (top2, dt2), (top3, dt3), (top4, dt4), (top5, dt5)] {
                        if let Some(tex) = maybe_tex {
                            draw_decor_texture(&mut frame, tex, &data.extents, src_offset, scale, tiling_shader)?;
                        }
                    }
                }
                _ => (),
            }

            if let Some(tex) = &text_tex
                && !self.title_text.extents.is_empty()
                && let Some(pixels) = &self.title_text_pixels
            {
                let text_logical_size: Size<i32, Logical> = (
                    (pixels.size.w as f64 / scale).round() as i32,
                    (pixels.size.h as f64 / scale).round() as i32,
                )
                    .into();
                let text_extents = Rectangle::new(self.title_text.extents.loc, text_logical_size);
                draw_texture(&mut frame, tex, &text_extents, None, scale, None)?;
            }

            for (btn, data) in [
                (TitlebarButton::Close, &self.close),
                (TitlebarButton::Hide, &self.hide),
                (TitlebarButton::Maximize, &self.maximize),
                (TitlebarButton::Menu, &self.menu),
                (TitlebarButton::Shade, &self.shade),
                (TitlebarButton::Stick, &self.stick),
            ] {
                if !data.extents.is_empty() {
                    let btn_name = DecorButtonName::from((btn, self.button_toggled_states));
                    let btn_state = self.button_state_for(btn, bg_state);
                    if let Some(tex) = self.decoration_theme.button_texture(btn_name, btn_state, bg_state) {
                        draw_decor_texture(&mut frame, tex, &data.extents, None, scale, tiling_shader)?;
                    }
                }
            }

            if let Some(tex) = &icon_tex
                && !self.window_icon_data.extents.is_empty()
            {
                draw_texture(&mut frame, tex, &self.window_icon_data.extents, None, scale, None)?;
            }

            let sync = frame.finish()?;
            renderer.wait(&sync)?;
            drop(fb);

            Ok(Some(offscreen))
        } else {
            Ok(None)
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

        let bg_state = if self.is_active {
            DecorBackgroundState::Active
        } else {
            DecorBackgroundState::Inactive
        };

        let tiling_shader = self.decoration_theme.tiling_shader();

        if self.titlebar_cache.texture.borrow().is_none() && !self.titlebar_cache.extents.is_empty() {
            match self.composite_titlebar(renderer, bg_state, tiling_shader) {
                Ok(texture) => *self.titlebar_cache.texture.borrow_mut() = texture,
                Err(err) => tracing::warn!("Failed to composite titlebar: {err}"),
            }
        }

        let titlebar_elem = {
            let tex = self.titlebar_cache.texture.borrow();
            if let Some(tex) = tex.as_ref()
                && !self.titlebar_cache.extents.is_empty()
            {
                let titlebar_location = location + self.titlebar_cache.extents.loc.to_f64().to_physical(scale);
                let tex_src = Rectangle::from_size((tex.size().w, tex.size().h).into()).to_f64();
                vec![DecorationRenderElement::Texture(TextureRenderElement::from_static_texture(
                    Id::new(),
                    renderer.context_id(),
                    titlebar_location,
                    tex.clone(),
                    buffer_scale,
                    Transform::Normal,
                    Some(alpha),
                    Some(tex_src),
                    Some(self.titlebar_cache.extents.size),
                    None,
                    Kind::Unspecified,
                ))]
            } else {
                Vec::new()
            }
        };

        let context_id = renderer.context_id();

        [
            titlebar_elem,
            create_render_elem(
                &context_id,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Left, bg_state),
                &self.left,
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
                &self.right,
                location,
                buffer_scale,
                scale,
                alpha,
                None,
            ),
            create_render_elem(
                &context_id,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Bottom, bg_state),
                &self.bottom,
                location,
                buffer_scale,
                scale,
                alpha,
                None,
            ),
            create_render_elem(
                &context_id,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::BottomLeft, bg_state),
                &self.bottom_left,
                location,
                buffer_scale,
                scale,
                alpha,
                None,
            ),
            create_render_elem(
                &context_id,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::BottomRight, bg_state),
                &self.bottom_right,
                location,
                buffer_scale,
                scale,
                alpha,
                None,
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

    #[allow(clippy::too_many_arguments)]
    pub fn enable_decorations(
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
        let mut decoration_state = self.decoration_state();
        if decoration_state.window_decorations.is_none() {
            let window_title = match self.0.underlying_surface() {
                WindowSurface::Wayland(toplevel_surface) => window_title_for_xdg_toplevel(toplevel_surface),
                WindowSurface::X11(x11_surface) => Some(x11_surface.title()),
            };

            decoration_state.window_decorations = Some(WindowDecorations::new(
                window_size,
                window_title,
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

    pub fn disable_decorations(&self) {
        self.decoration_state().window_decorations = None;
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn enable_decorations_for_window(&mut self, window: &WindowElement) {
        let window_size = SpaceElement::geometry(&window.0).size;

        let scale = self
            .core
            .workspace_manager
            .outputs_for_element(window)
            .first()
            .or_else(|| self.core.workspace_manager.outputs().next())
            .map(|output| output.current_scale())
            .unwrap_or(OutputScale::Integer(1));
        let window_icon = match window.0.underlying_surface() {
            WindowSurface::Wayland(toplevel_surface) => {
                let app_info = desktop_app_info_for_xdg_toplevel(toplevel_surface);
                icon_for_xdg_toplevel(toplevel_surface, scale.integer_scale(), app_info.as_ref())
                    .and_then(|icon| self.window_icon_to_image_data(&icon).ok())
            }
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
    }
}

#[allow(clippy::too_many_arguments)]
fn create_render_elem(
    context_id: &ContextId<GlesTexture>,
    tiling_shader: &GlesTexProgram,
    texture: &DecorTexture,
    texture_data: &TextureData,
    location_offset: Point<f64, Physical>,
    buffer_scale: i32,
    scale: Scale<f64>,
    alpha: f32,
    src_offset: Option<Point<i32, Buffer>>,
) -> Vec<DecorationRenderElement> {
    if texture_data.extents.is_empty() {
        vec![]
    } else {
        let location = location_offset + texture_data.extents.loc.to_f64().to_physical(scale);
        vec![match texture.rendering_mode() {
            DecorRenderingMode::Tiled(direction) => DecorationRenderElement::TiledTexture(create_tiled_texture_elem(
                context_id,
                texture_data.id.clone(),
                texture,
                tiling_shader,
                location,
                texture_data.extents.size,
                buffer_scale,
                alpha,
                direction,
                src_offset,
            )),
            DecorRenderingMode::Stretched(_) => DecorationRenderElement::Texture(create_texture_elem(
                context_id,
                texture_data.id.clone(),
                texture,
                location,
                texture_data.extents.size,
                buffer_scale,
                alpha,
                src_offset,
            )),
            DecorRenderingMode::AsIs => DecorationRenderElement::Texture(create_texture_elem(
                context_id,
                texture_data.id.clone(),
                texture,
                location,
                texture_data.extents.size,
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
    buffer_scale: i32,
    alpha: f32,
    direction: Direction,
    src_offset: Option<Point<i32, Buffer>>,
) -> TextureShaderElement {
    let element = create_texture_elem(context_id, id, texture, location, render_size, buffer_scale, alpha, src_offset);

    let tex_size = texture.size().to_f64();
    let geo_size = render_size.to_f64();

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

fn draw_decor_texture(
    frame: &mut GlesFrame<'_, '_>,
    texture: &DecorTexture,
    extents: &Rectangle<i32, Logical>,
    src_offset: Option<Point<i32, Buffer>>,
    scale: f64,
    tiling_shader: &GlesTexProgram,
) -> anyhow::Result<()> {
    let tiling = match texture.rendering_mode() {
        DecorRenderingMode::Tiled(direction) => Some((direction, tiling_shader)),
        _ => None,
    };
    draw_texture(frame, texture, extents, src_offset, scale, tiling)
}

fn draw_texture(
    frame: &mut GlesFrame<'_, '_>,
    texture: &GlesTexture,
    extents: &Rectangle<i32, Logical>,
    src_offset: Option<Point<i32, Buffer>>,
    scale: f64,
    tiling: Option<(Direction, &GlesTexProgram)>,
) -> anyhow::Result<()> {
    if !extents.is_empty() {
        let tex_size = texture.size();
        let src: Rectangle<f64, Buffer> = if let Some(offset) = src_offset {
            Rectangle::new((offset.x, offset.y).into(), (tex_size.w - offset.x, tex_size.h - offset.y).into())
        } else {
            Rectangle::from_size(tex_size)
        }
        .to_f64();

        let dest: Rectangle<i32, Physical> = {
            // We need to scale and round the edges in order to avoid rounding issues that can
            // result in the textures being 1 pixel too narrow sometimes, depending on the size of
            // the texture.
            let loc = extents.loc.to_f64().to_physical(scale).to_i32_round();
            let end = (extents.loc + extents.size).to_f64().to_physical(scale).to_i32_round();
            let size = end - loc;
            Rectangle::new(loc, (size.x, size.y).into())
        };

        let uniforms = tiling.as_ref().map(|(direction, _)| {
            let tile_mask = match direction {
                Direction::Horizontal => (1.0f32, 0.0f32),
                Direction::Vertical => (0.0f32, 1.0f32),
            };

            vec![
                Uniform::new("tex_size", UniformValue::_2f(tex_size.w as f32, tex_size.h as f32)),
                Uniform::new("geo_size", UniformValue::_2f(dest.size.w as f32, dest.size.h as f32)),
                Uniform::new("tile_mask", UniformValue::_2f(tile_mask.0, tile_mask.1)),
            ]
        });
        let tiling_shader = tiling.map(|(_, shader)| shader);

        let damage = [Rectangle::from_size(dest.size)];
        frame.render_texture_from_to(
            texture,
            src,
            dest,
            &damage,
            &[],
            Transform::Normal,
            1.0,
            tiling_shader,
            uniforms.as_deref().unwrap_or(&[]),
        )?;
    }
    Ok(())
}

fn render_title_text_pixels(
    layout: pango::Layout,
    extents: Rectangle<i32, Physical>,
    max_width: f64,
    config: &Xfwl4Config,
    state: DecorBackgroundState,
) -> Option<PixelBuffer> {
    profiling::scope!("render_title_text_pixels");
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, extents.size.w, extents.size.h).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;

    cr.rectangle(0., 0., max_width, extents.size.h as f64);
    cr.clip();
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
        pangocairo::functions::show_layout(&cr, &layout);
    }

    drop(cr);

    let width = surface.width();
    let height = surface.height();
    let src = surface.data().ok()?;
    let data: Vec<u8> = src.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    Some(PixelBuffer {
        data,
        size: (width, height).into(),
        format: Fourcc::Abgr8888,
    })
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

fn pixbuf_to_pixels(pixbuf: &gdk_pixbuf::Pixbuf) -> Option<PixelBuffer> {
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf.clone()
    } else {
        pixbuf.add_alpha(false, 0, 0, 0).ok()?
    };

    let width = pixbuf.width() as usize;
    let height = pixbuf.height() as usize;
    let stride = pixbuf.rowstride() as usize;
    let src = pixbuf.read_pixel_bytes();

    let mut data = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let s = y * stride + x * 4;
            let d = (y * width + x) * 4;
            let a = src[s + 3] as u32;
            data[d] = ((src[s] as u32 * a + 127) / 255) as u8;
            data[d + 1] = ((src[s + 1] as u32 * a + 127) / 255) as u8;
            data[d + 2] = ((src[s + 2] as u32 * a + 127) / 255) as u8;
            data[d + 3] = src[s + 3];
        }
    }

    Some(PixelBuffer {
        data,
        size: (width as i32, height as i32).into(),
        format: Fourcc::Abgr8888,
    })
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

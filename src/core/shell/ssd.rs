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
    input::Seat,
    output::Scale as OutputScale,
    render_elements,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform},
};

use std::{
    cell::{RefCell, RefMut},
    collections::HashSet,
    time::Duration,
};

use crate::{
    backend::Backend,
    core::{
        config::{DoubleClickAction, TitleAlignment, TitlebarButton, Xfwl4Config},
        cursor::CursorName,
        drawing::{
            decorations::{
                DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode, DecorTexture,
                DecorTitleTextures, DecorationTheme, Direction,
            },
            shadows::{ShadowKey, ShadowParams},
            ssd::{DecorationRenderState, create_title_layout, render_title_text_pixels},
        },
        shell::{
            GrabTrigger, ResizeEdge,
            xdg::{desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel, window_title_for_xdg_toplevel},
        },
        state::Xfwl4State,
        ui_thread::ActionLocation,
        util::{BTN_LEFT, BTN_RIGHT, ImageData, ScrollAccumulator, icon_theme::FreedesktopIconsIconTheme},
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

fn point_in_rect(rect: &Rectangle<i32, Logical>, loc: Point<f64, Logical>) -> bool {
    !rect.is_empty() && rect.contains(loc.to_i32_ceil())
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub(in crate::core) enum TitleLayout {
    TitleStretched {
        extents: Rectangle<i32, Logical>,
    },
    Title5Part {
        title1: Rectangle<i32, Logical>,
        top1: Rectangle<i32, Logical>,
        title2: Rectangle<i32, Logical>,
        top2: Rectangle<i32, Logical>,
        title3: Rectangle<i32, Logical>,
        top3: Rectangle<i32, Logical>,
        title4: Rectangle<i32, Logical>,
        top4: Rectangle<i32, Logical>,
        title5: Rectangle<i32, Logical>,
        top5: Rectangle<i32, Logical>,
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

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct DirtyFlags: u8 {
        const TITLEBAR = 1;
        const TITLE_TEXT = 2;
    }
}

#[derive(Debug, Clone)]
pub(in crate::core) struct DecorationLayout {
    pub top_left: Rectangle<i32, Logical>,
    pub top: Rectangle<i32, Logical>,
    pub top_right: Rectangle<i32, Logical>,
    pub bottom_left: Rectangle<i32, Logical>,
    pub bottom: Rectangle<i32, Logical>,
    pub bottom_right: Rectangle<i32, Logical>,
    pub left: Rectangle<i32, Logical>,
    pub right: Rectangle<i32, Logical>,
    pub close: Rectangle<i32, Logical>,
    pub hide: Rectangle<i32, Logical>,
    pub maximize: Rectangle<i32, Logical>,
    pub menu: Rectangle<i32, Logical>,
    pub shade: Rectangle<i32, Logical>,
    pub stick: Rectangle<i32, Logical>,
    pub title_text: Rectangle<i32, Logical>,
    pub title_text_max_width: i32,
    pub titlebar: Rectangle<i32, Logical>,
    pub title: TitleLayout,
    pub top_clip: i32,
    pub shadow_offset: Point<i32, Logical>,
    pub shadow_size: Size<i32, Logical>,
    pub shadow_frame_size: Size<i32, Logical>,
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
    titlebar_double_click_state: Option<DoubleClickState>,

    window_icon: Option<ImageData>,

    title_text_logical_size: Size<i32, Logical>,

    layout: DecorationLayout,
    render_state: DecorationRenderState,
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
            is_active: false,
            hover_state: HoverState::None,
            pressed_state: PressedState::None,
            button_toggled_states: ButtonToggledStates::empty(),
            scroll_accumulator: ScrollAccumulator::default(),
            titlebar_double_click_state: None,
            window_icon,
            title_text_logical_size: Size::default(),
            layout: DecorationLayout {
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
            },
            render_state: DecorationRenderState::new(),
        };
        let flags = decorations.recalculate_layout();
        decorations.invalidate_render_state(flags | DirtyFlags::TITLE_TEXT);
        decorations
    }

    pub fn point_is_in_decorations(&self, location: Point<f64, Logical>) -> bool {
        let location = location.to_i32_ceil();
        let in_title = match &self.layout.title {
            TitleLayout::TitleStretched { extents } => !extents.is_empty() && extents.contains(location),
            TitleLayout::Title5Part {
                title1,
                title2,
                title3,
                title4,
                title5,
                ..
            } => {
                (!title1.is_empty() && title1.contains(location))
                    || (!title2.is_empty() && title2.contains(location))
                    || (!title3.is_empty() && title3.contains(location))
                    || (!title4.is_empty() && title4.contains(location))
                    || (!title5.is_empty() && title5.contains(location))
            }
        };
        if in_title {
            return true;
        }

        for rect in [
            &self.layout.top_left,
            &self.layout.top_right,
            &self.layout.bottom_left,
            &self.layout.bottom_right,
            &self.layout.left,
            &self.layout.right,
            &self.layout.bottom,
        ] {
            if !rect.is_empty() && rect.contains(location) {
                return true;
            }
        }

        false
    }

    pub fn left_decoration_width(&self) -> i32 {
        self.layout.left.size.w
    }

    pub fn right_decoration_width(&self) -> i32 {
        self.layout.right.size.w
    }

    pub fn top_decoration_height(&self) -> i32 {
        match &self.layout.title {
            TitleLayout::TitleStretched { extents } => extents.size.h,
            TitleLayout::Title5Part { title3, .. } => title3.size.h,
        }
    }

    pub fn bottom_decoration_height(&self) -> i32 {
        self.layout.bottom.size.h
    }

    pub fn decorations_offset(&self) -> Point<i32, Logical> {
        (self.left_decoration_width(), self.top_decoration_height()).into()
    }

    pub fn shadow_extents(&self) -> (i32, i32, i32, i32) {
        if self.layout.shadow_size.is_empty() {
            (0, 0, 0, 0)
        } else {
            let left = (-self.layout.shadow_offset.x).max(0);
            let top = (-self.layout.shadow_offset.y).max(0);
            let right = (self.layout.shadow_offset.x + self.layout.shadow_size.w - self.layout.shadow_frame_size.w).max(0);
            let bottom = (self.layout.shadow_offset.y + self.layout.shadow_size.h - self.layout.shadow_frame_size.h).max(0);
            (left, top, right, bottom)
        }
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
            (&self.layout.close, HoverState::Close),
            (&self.layout.hide, HoverState::Hide),
            (&self.layout.maximize, HoverState::Maximize),
            (&self.layout.menu, HoverState::Menu),
            (&self.layout.shade, HoverState::Shade),
            (&self.layout.stick, HoverState::Stick),
        ];

        let new_hover_state = buttons
            .iter_mut()
            .find_map(|(rect, flag)| point_in_rect(rect, loc).then_some(*flag))
            .unwrap_or(HoverState::None);

        if new_hover_state != HoverState::None {
            if new_hover_state != self.hover_state {
                self.hover_state = new_hover_state;
                self.invalidate_render_state(DirtyFlags::TITLEBAR);
            }
            state.core.set_cursor(CursorName::Default);
        } else {
            let resize_grips = [
                (&self.layout.top_left, HoverState::TopLeft, CursorName::TopLeftCorner),
                (&self.layout.top, HoverState::Top, CursorName::TopSide),
                (&self.layout.top_right, HoverState::TopRight, CursorName::TopRightCorner),
                (&self.layout.left, HoverState::Left, CursorName::LeftSide),
                (&self.layout.right, HoverState::Right, CursorName::RightSide),
                (&self.layout.bottom_left, HoverState::BottomLeft, CursorName::BottomLeftCorner),
                (&self.layout.bottom, HoverState::Bottom, CursorName::BottomSide),
                (&self.layout.bottom_right, HoverState::BottomRight, CursorName::BottomRightCorner),
                (&self.layout.titlebar, HoverState::Titlebar, CursorName::Default),
            ];

            let (new_hover_state, new_cursor_name) = resize_grips
                .iter()
                .find_map(|(rect, flag, cursor)| point_in_rect(rect, loc).then_some((*flag, *cursor)))
                .unwrap_or((HoverState::None, CursorName::Default));

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
            _ if !is_button_hover(self.hover_state) => state.core.set_cursor(CursorName::Default),
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
        if let Some(pointer_loc) = self.pointer_loc.as_ref() {
            let buttons = [
                (&self.layout.close, PressedState::Close),
                (&self.layout.hide, PressedState::Hide),
                (&self.layout.maximize, PressedState::Maximize),
                (&self.layout.menu, PressedState::Menu),
                (&self.layout.shade, PressedState::Shade),
                (&self.layout.stick, PressedState::Stick),
            ];

            let new_pressed_state = buttons
                .iter()
                .find_map(|(rect, flag)| point_in_rect(rect, *pointer_loc).then_some(*flag))
                .unwrap_or(PressedState::None);

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
                let titlebar_parts: Vec<(&Rectangle<i32, Logical>, PressedState)> = match &self.layout.title {
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

                let resize_grips: [(&Rectangle<i32, Logical>, PressedState); 8] = [
                    (&self.layout.top_left, PressedState::TopLeft),
                    (&self.layout.top, PressedState::Top),
                    (&self.layout.top_right, PressedState::TopRight),
                    (&self.layout.left, PressedState::Left),
                    (&self.layout.right, PressedState::Right),
                    (&self.layout.bottom_left, PressedState::BottomLeft),
                    (&self.layout.bottom, PressedState::Bottom),
                    (&self.layout.bottom_right, PressedState::BottomRight),
                ];

                let mut move_resize_grips = resize_grips.into_iter().chain(titlebar_parts);

                let new_pressed_state = move_resize_grips
                    .find_map(|(rect, flag)| point_in_rect(rect, *pointer_loc).then_some(flag))
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

            if let Some(pointer_loc) = self.pointer_loc.as_ref() {
                let buttons = [
                    (&self.layout.close, PressedState::Close),
                    (&self.layout.hide, PressedState::Hide),
                    (&self.layout.maximize, PressedState::Maximize),
                    (&self.layout.menu, PressedState::Menu),
                    (&self.layout.shade, PressedState::Shade),
                    (&self.layout.stick, PressedState::Stick),
                    (&self.layout.titlebar, PressedState::Titlebar),
                ];

                let final_pressed_state = buttons
                    .iter()
                    .find_map(|(rect, flag)| point_in_rect(rect, *pointer_loc).then_some(*flag));
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
                        PressedState::Stick => {
                            // Use an idle function here because we otherwise end up recursively trying
                            // to borrow the RefCell that WindowDecorations (aka 'self') is in, and
                            // crash.
                            let window = window.clone();
                            let new_is_sticky = !self.button_toggled_states.contains(ButtonToggledStates::Stick);
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_sticky(&window, new_is_sticky);
                            });
                        }
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
                        PressedState::Titlebar => self.handle_titlebar_double_click(state, window, button, time, *pointer_loc),
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
        let other_parts_to_ignore = [&self.layout.top_left, &self.layout.top, &self.layout.top_right];

        let double_click_action = self.config.double_click_action();
        if double_click_action != DoubleClickAction::None
            && button == BTN_LEFT
            && !other_parts_to_ignore.iter().any(|part| point_in_rect(part, pointer_loc))
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
                                state.set_window_maximized(&window, !window.maximized());
                            });
                        }
                        DoubleClickAction::Fill => {
                            // Use an idle function here because we otherwise end up recursively trying
                            // to borrow the RefCell that WindowDecorations (aka 'self') is in, and
                            // crash.
                            let window = window.clone();
                            state.core.handle.insert_idle(move |state| {
                                state.set_window_filled(&window);
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

    pub fn update_window_title(&mut self, window_title: &str) {
        let window_title = Some(window_title.to_owned());
        if self.window_title != window_title {
            self.window_title = window_title;
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

    fn recalculate_layout(&mut self) -> DirtyFlags {
        profiling::scope!("WindowDecorations::recalculate_layout");
        if self.window_size.w <= 0 || self.window_size.h <= 0 {
            return DirtyFlags::empty();
        }

        let old_titlebar_size = self.layout.titlebar.size;

        let bg_state = if self.is_active {
            DecorBackgroundState::Active
        } else {
            DecorBackgroundState::Inactive
        };

        let borderless_maximize = self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize();

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
        self.layout.top_clip = top_clip;
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
                    .to_logical(1, Transform::Normal)
                    .w,
                self.decoration_theme
                    .background_texture(DecorBackgroundName::Right, bg_state)
                    .size()
                    .to_logical(1, Transform::Normal)
                    .w,
                bottom_strip.bottom_extents.size.to_logical(1, Transform::Normal).h,
                self.decoration_theme
                    .background_texture(DecorBackgroundName::TopLeft, bg_state)
                    .size()
                    .to_logical(1, Transform::Normal),
                self.decoration_theme
                    .background_texture(DecorBackgroundName::TopRight, bg_state)
                    .size()
                    .to_logical(1, Transform::Normal),
                bottom_strip.bottom_left_extents.size.to_logical(1, Transform::Normal),
                bottom_strip.bottom_right_extents.size.to_logical(1, Transform::Normal),
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

        self.layout.top_left = Rectangle::new((0, 0).into(), corner_top_left_size);
        self.layout.top_right = Rectangle::new((total_frame_size.w - corner_top_right_size.w, 0).into(), corner_top_right_size);
        self.layout.top = Rectangle::new(
            (corner_top_left_size.w, 0).into(),
            (
                (total_frame_size.w - corner_top_left_size.w - corner_top_right_size.w).max(0),
                frame_bottom_h, // Make the top resize grip area the same height as the bottom
            )
                .into(),
        );

        self.layout.bottom_left = Rectangle::new((0, total_frame_size.h - corner_bottom_left_size.h).into(), corner_bottom_left_size);
        self.layout.bottom_right = Rectangle::new((total_frame_size - corner_bottom_right_size).to_point(), corner_bottom_right_size);
        self.layout.bottom = Rectangle::new(
            (corner_bottom_left_size.w, total_frame_size.h - frame_bottom_h).into(),
            (
                (total_frame_size.w - corner_bottom_left_size.w - corner_bottom_right_size.w).max(0),
                frame_bottom_h,
            )
                .into(),
        );

        if borderless_maximize || self.button_toggled_states.contains(ButtonToggledStates::Shade) {
            self.layout.left = Rectangle::zero();
            self.layout.right = Rectangle::zero();
        } else {
            self.layout.left = Rectangle::new(
                (0, visible_top_h).into(),
                (
                    frame_left_w,
                    (self.window_size.h + frame_bottom_h - corner_bottom_left_size.h).max(0),
                )
                    .into(),
            );
            self.layout.right = Rectangle::new(
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

            let title_height = self.title_text_logical_size.h;
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
                    TitleAlignment::Right => w3 - self.title_text_logical_size.w - self.config.title_horizontal_offset(),
                    TitleAlignment::Center => (w3 / 2) - (self.title_text_logical_size.w / 2),
                }
                .max(self.config.title_horizontal_offset());
            } else {
                let title_shadow = if bg_state == DecorBackgroundState::Active {
                    self.config.title_shadow_active()
                } else {
                    self.config.title_shadow_inactive()
                } as i32; // FIXME: this seems wrong
                w3 = (self.title_text_logical_size.w + title_shadow)
                    .min(frame_top_size.w - w2 - w4)
                    .max(0);

                w1 = match self.config.title_alignment() {
                    TitleAlignment::Left => btn_left + self.config.title_horizontal_offset(),
                    TitleAlignment::Right => btn_right - w2 - w3 - w4 - self.config.title_horizontal_offset(),
                    TitleAlignment::Center => btn_left + ((btn_right - btn_left) / 2) - (w3 / 2) - w2,
                }
                .max(btn_left);
            }

            match &title_bg_textures {
                DecorTitleTextures::TitleStretched(_) => {
                    if let TitleLayout::Title5Part { .. } = &self.layout.title {
                        self.layout.title = TitleLayout::TitleStretched {
                            extents: Rectangle::zero(),
                        };
                    }
                }
                DecorTitleTextures::Title5Part { .. } => {
                    if let TitleLayout::TitleStretched { .. } = &self.layout.title {
                        self.layout.title = TitleLayout::Title5Part {
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
            match (&title_bg_textures, &mut self.layout.title) {
                (DecorTitleTextures::TitleStretched(_), TitleLayout::TitleStretched { extents }) => {
                    *extents = Rectangle::new((corner_top_left_size.w + x, 0).into(), (frame_top_size.w, visible_top_h).into());

                    title_x = hoffset + w1 + w2;
                    let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                    self.layout.title_text_max_width = title_max_width;
                    self.layout.title_text = Rectangle::new(
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

                    self.layout.title_text = if w3 > 0 {
                        *title3_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_h).into());
                        *top3_ext = Rectangle::new((corner_top_left_size.w + x, 0).into(), (w3, visible_top_height).into());
                        title_x = hoffset + x;
                        x += w3;

                        let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                        self.layout.title_text_max_width = title_max_width;

                        Rectangle::new(
                            (corner_top_left_size.w + title_x, title_y).into(),
                            (btn_right - w4, visible_top_h).into(),
                        )
                    } else {
                        *title3_ext = Rectangle::zero();
                        *top3_ext = Rectangle::zero();
                        self.layout.title_text_max_width = 0;
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
            self.layout.shadow_offset = sp.offset;
            self.layout.shadow_size = sp.size;
            self.layout.shadow_frame_size = total_frame_size;
        } else {
            self.layout.shadow_offset = Point::default();
            self.layout.shadow_size = Size::default();
            self.layout.shadow_frame_size = Size::default();
            self.render_state.shadow_cache.clear();
        }

        self.layout.titlebar = Rectangle::new((0, 0).into(), (total_frame_size.w, visible_top_h).into());

        let mut flags = DirtyFlags::empty();
        if self.layout.titlebar.size != old_titlebar_size {
            flags |= DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT;
        }
        flags
    }

    fn invalidate_render_state(&mut self, flags: DirtyFlags) {
        if flags.contains(DirtyFlags::TITLE_TEXT) {
            profiling::scope!("invalidate_title_text");
            let bg_state = if self.is_active {
                DecorBackgroundState::Active
            } else {
                DecorBackgroundState::Inactive
            };
            let scale = self.scale.fractional_scale();
            let (layout, title_extents) = create_title_layout(
                &self.font_map,
                &self.font_options,
                self.window_title.as_deref(),
                &self.config.title_font(),
                scale,
            );
            self.title_text_logical_size = title_extents.size.to_f64().to_logical(scale).to_i32_round();
            let max_width = self.layout.title_text_max_width as f64 * scale;
            self.render_state.title_text_pixels = render_title_text_pixels(layout, title_extents, max_width, &self.config, bg_state);
        }

        if flags.intersects(DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT) {
            self.render_state
                .load_window_icon(&self.layout.menu, self.window_icon.as_ref(), &self.icon_theme);
            self.render_state.invalidate_titlebar();
        }
    }

    #[inline]
    fn extents_for_button_mut(&mut self, btn: TitlebarButton) -> &mut Rectangle<i32, Logical> {
        match btn {
            TitlebarButton::Menu => &mut self.layout.menu,
            TitlebarButton::Hide => &mut self.layout.hide,
            TitlebarButton::Stick => &mut self.layout.stick,
            TitlebarButton::Shade => &mut self.layout.shade,
            TitlebarButton::Close => &mut self.layout.close,
            TitlebarButton::Maximize => &mut self.layout.maximize,
            TitlebarButton::SideSeparator => unreachable!(),
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

        let bg_state = if self.is_active {
            DecorBackgroundState::Active
        } else {
            DecorBackgroundState::Inactive
        };

        let tiling_shader = self.decoration_theme.tiling_shader();

        if self.render_state.titlebar_texture.borrow().is_none() && !self.layout.titlebar.is_empty() {
            match self.render_state.composite_titlebar(
                renderer,
                bg_state,
                tiling_shader,
                &self.layout,
                &self.decoration_theme,
                self.scale.fractional_scale(),
                self.button_toggled_states,
                self.hover_state,
                self.pressed_state,
            ) {
                Ok(texture) => *self.render_state.titlebar_texture.borrow_mut() = texture,
                Err(err) => tracing::warn!("Failed to composite titlebar: {err}"),
            }
        }

        let titlebar_elem = {
            let tex = self.render_state.titlebar_texture.borrow();
            if let Some(tex) = tex.as_ref()
                && !self.layout.titlebar.is_empty()
            {
                let titlebar_location = location + self.layout.titlebar.loc.to_f64().to_physical(scale);
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
                    Some(self.layout.titlebar.size),
                    None,
                    Kind::Unspecified,
                ))]
            } else {
                Vec::new()
            }
        };

        let context_id = renderer.context_id();

        let bottom_strip_elem = {
            let bottom_strip = self.decoration_theme.bottom_background_texture(bg_state);
            let bottom_strip_location = location + self.layout.bottom_left.loc.to_f64().to_physical(scale);
            let render_size = Size::<_, Logical>::new(
                self.layout.bottom_left.size.w + self.layout.bottom.size.w + self.layout.bottom_right.size.w,
                self.layout
                    .bottom_left
                    .size
                    .h
                    .max(self.layout.bottom.size.h)
                    .max(self.layout.bottom_right.size.h),
            );
            vec![DecorationRenderElement::TiledTexture(create_tiled_texture_elem_with_margin(
                &context_id,
                self.render_state.bottom_id.clone(),
                bottom_strip.texture,
                tiling_shader,
                bottom_strip_location,
                render_size,
                buffer_scale,
                alpha,
                Direction::Horizontal,
                None,
                bottom_strip.bottom_left_extents.size.w,
                bottom_strip.bottom_right_extents.size.w,
            ))]
        };

        let shadow_elem = if !self.layout.shadow_size.is_empty() {
            profiling::scope!("ensure_shadow_texture");
            self.render_state
                .ensure_shadow_texture(renderer, &self.config, self.layout.shadow_frame_size);
            let key = ShadowKey::from_config(&self.config, self.layout.shadow_frame_size);
            if let Some(shadow_tex) = self.render_state.shadow_cache.get(key) {
                let shadow_location = location + shadow_tex.offset.to_f64().to_physical(scale);
                vec![DecorationRenderElement::Texture(shadow_tex.render_element(
                    renderer,
                    shadow_location,
                    alpha,
                ))]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        [
            titlebar_elem,
            create_render_elem(
                &context_id,
                tiling_shader,
                self.decoration_theme.background_texture(DecorBackgroundName::Left, bg_state),
                &self.render_state.left_id,
                &self.layout.left,
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
                &self.layout.right,
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
    pub(in crate::core) fn enable_decorations_for_window(&mut self, window: &WindowElement) {
        let window_size = SpaceElement::geometry(&window.0).size;

        let scale = self
            .core
            .workspace_manager
            .outputs_for_window(window)
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
    id: &Id,
    extents: &Rectangle<i32, Logical>,
    location_offset: Point<f64, Physical>,
    buffer_scale: i32,
    scale: Scale<f64>,
    alpha: f32,
    src_offset: Option<Point<i32, Buffer>>,
) -> Vec<DecorationRenderElement> {
    if extents.is_empty() {
        vec![]
    } else {
        let location = location_offset + extents.loc.to_f64().to_physical(scale);
        vec![match texture.rendering_mode() {
            DecorRenderingMode::Tiled(direction) => DecorationRenderElement::TiledTexture(create_tiled_texture_elem(
                context_id,
                id.clone(),
                texture,
                tiling_shader,
                location,
                extents.size,
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
                extents.size,
                buffer_scale,
                alpha,
                src_offset,
            )),
            DecorRenderingMode::AsIs => DecorationRenderElement::Texture(create_texture_elem(
                context_id,
                id.clone(),
                texture,
                location,
                extents.size,
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
    create_tiled_texture_elem_with_margin(
        context_id,
        id,
        texture,
        shader,
        location,
        render_size,
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
    buffer_scale: i32,
    alpha: f32,
    direction: Direction,
    src_offset: Option<Point<i32, Buffer>>,
    margin_left: i32,
    margin_right: i32,
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
        Uniform::new("margin_left", UniformValue::_1f(margin_left as f32)),
        Uniform::new("margin_right", UniformValue::_1f(margin_right as f32)),
    ]
    .to_vec();

    TextureShaderElement::new(element, shader.clone(), uniforms)
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

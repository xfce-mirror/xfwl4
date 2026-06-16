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
    collections::HashMap,
    time::Duration,
};

use crate::{
    backend::Backend,
    core::{
        config::{DoubleClickAction, TitleAlignment, TitlebarButton, Xfwl4Config},
        drawing::{
            decorations::{
                BottomTexture, DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode,
                DecorTexture, DecorTitleTextures, DecorationTheme, Direction,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum Corner {
    TopLeft,
    TopRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum BottomSlot {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum TitleSlot {
    Stretched,
    Title1,
    Top1,
    Title2,
    Top2,
    Title3,
    Top3,
    Title4,
    Top4,
    Title5,
    Top5,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum PieceRole {
    Corner(Corner),
    TitlePart(TitleSlot),
    Button(TitlebarButton),
    Side(Side),
    BottomSlice(BottomSlot),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::core) enum FrameSection {
    Titlebar,
    Side,
    Bottom,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::core) struct PlacedPiece {
    pub role: PieceRole,
    pub placement: Rectangle<i32, Physical>,
    pub src_offset: Point<i32, Buffer>,
    pub section: FrameSection,
}

struct PieceInputSource<'a> {
    opaque_regions: &'a [Rectangle<i32, Buffer>],
    tex_size: Size<i32, Buffer>,
    mode: DecorRenderingMode,
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
    pub title_text: Rectangle<i32, Physical>,
    pub title_text_max_width: i32,
    pub titlebar: Rectangle<i32, Physical>,
    pub top_clip: i32,
    pub shadow_offset: Point<i32, Physical>,
    pub shadow_size: Size<i32, Physical>,
    pub shadow_frame_size: Size<i32, Physical>,
    pub pieces: Vec<PlacedPiece>,
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
            title_text: Rectangle::zero(),
            title_text_max_width: 0,
            titlebar: Rectangle::zero(),
            top_clip: 0,
            shadow_offset: Point::default(),
            shadow_size: Size::default(),
            shadow_frame_size: Size::default(),
            pieces: Vec::new(),
        }
    }
}

// The scale-free (native theme-bitmap px) part of a frame's geometry: border widths, titlebar
// height, corner sizes and the maximize-driven flags.  Only the content span scales per output, so
// these feed both `build_layout` (any scale) and the window's logical extents reported to the Space
// without choosing a scale.
#[derive(Debug, Clone, Copy, Default)]
struct FrameMetrics {
    is_shaded: bool,
    borderless_maximize: bool,
    top_clip: i32,
    frame_top_h: i32,
    visible_top_h: i32,
    frame_left_w: i32,
    frame_right_w: i32,
    frame_bottom_h: i32,
    corner_top_left_size: Size<i32, Physical>,
    corner_top_right_size: Size<i32, Physical>,
    corner_bottom_left_size: Size<i32, Physical>,
    corner_bottom_right_size: Size<i32, Physical>,
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
    // The titlebar's opaque pixels in the composited titlebar texture's buffer coords (= the input
    // region clipped to the titlebar), handed to smithay so the frame occludes windows behind it.
    titlebar_opaque_region: Vec<Rectangle<i32, Buffer>>,
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

    metrics: FrameMetrics,
    last_titlebar_size: Size<i32, Physical>,
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
            metrics: FrameMetrics::default(),
            last_titlebar_size: Size::default(),
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
        layout.pieces.iter().find_map(|piece| match piece.role {
            PieceRole::Button(button) if self.point_on_button(button, piece.placement, point) => Some(button),
            _ => None,
        })
    }

    // Returns the `ScaledRender` for `scale`, building it on demand.  The non-GPU parts (layout,
    // input region, title text) need no renderer, so the entry can be materialized off the render
    // path -- e.g. for hit-testing a window that has not been rendered yet.
    fn scaled_entry(&self, scale: f64) -> Ref<'_, ScaledRender> {
        let key = ScaleKey::new(scale);
        if !self.scaled.borrow().contains_key(&key) {
            let entry = self.build_scaled_render(key.scale());
            self.scaled.borrow_mut().insert(key, entry);
        }
        Ref::map(self.scaled.borrow(), |scaled| scaled.get(&key).expect("entry was just inserted"))
    }

    // Tested against the input region the pointer's output rendered (decorations are native px per
    // output); `scale` is that output's scale, supplied by the caller, which knows the output.  The
    // region is the union of every piece's opaque pixels, so transparent gaps in the titlebar and
    // borders fall through rather than swallowing the click.
    pub fn point_is_in_decorations(&self, location: Point<f64, Logical>, scale: f64) -> bool {
        let entry = self.scaled_entry(scale);
        Self::point_in_region(location, scale, &entry.input_region)
    }

    // For `is_in_input_region`, which has no output context: a point is in the decorations if it
    // lands on them at any scale the window is currently rendered at (i.e. any output it spans).
    // The precise per-output decoration/content split is left to `surface_under`.
    pub fn point_is_in_any_decorations(&self, location: Point<f64, Logical>) -> bool {
        self.scaled_entry(self.scale.fractional_scale());
        self.scaled
            .borrow()
            .iter()
            .any(|(key, entry)| Self::point_in_region(location, key.scale(), &entry.input_region))
    }

    pub fn decorations_extents_physical(&self) -> FrameExtents<i32, Physical> {
        FrameExtents::new(
            self.metrics.frame_left_w,
            self.metrics.frame_right_w,
            self.metrics.visible_top_h,
            self.metrics.frame_bottom_h,
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

    fn shadow_active(&self) -> bool {
        self.window_size.w > 0
            && self.window_size.h > 0
            && self.config.show_frame_shadow()
            && !self.button_toggled_states.contains(ButtonToggledStates::Maximize)
    }

    // The shadow's overhang past the frame, in native px.  The frame size cancels out of
    // `ShadowParams`' offset/size (only the config deltas and the gaussian blur remain), so this is
    // scale-free and needs no built layout -- a zero surface size yields the same extents.
    fn shadow_extents_physical(&self) -> FrameExtents<i32, Physical> {
        if self.shadow_active() {
            let params = ShadowParams::new(
                (self.config.shadow_delta_x(), self.config.shadow_delta_y()).into(),
                self.config.shadow_delta_width(),
                self.config.shadow_delta_height(),
                Size::default(),
            );
            let left = (-params.offset.x).max(0);
            let top = (-params.offset.y).max(0);
            let right = (params.offset.x + params.size.w).max(0);
            let bottom = (params.offset.y + params.size.h).max(0);
            FrameExtents::new(left, right, top, bottom)
        } else {
            FrameExtents::new(0, 0, 0, 0)
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
        (self.scaled_entry(scale).layout.clone(), scale)
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
                let titlebar_parts = layout
                    .pieces
                    .iter()
                    .filter(|piece| matches!(piece.role, PieceRole::TitlePart(_)))
                    .map(|piece| (&piece.placement, PressedState::Titlebar));

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

                let new_pressed_state = resize_grips
                    .into_iter()
                    .chain(titlebar_parts)
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

                let final_pressed_state = layout
                    .pieces
                    .iter()
                    .filter_map(|piece| match piece.role {
                        PieceRole::Button(button) => Some((&piece.placement, PressedState::from(button))),
                        _ => None,
                    })
                    .chain(std::iter::once((&layout.titlebar, PressedState::Titlebar)))
                    .find_map(|(rect, flag)| point_in_rect(rect, pointer_loc_physical).then_some(flag))
                    .unwrap_or(PressedState::None);

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

    // The titlebar size at the primary (decoration) scale: native border widths plus the scaled
    // content width, and the native titlebar height.  Used for resize change-detection without
    // building a full per-scale entry.
    fn primary_titlebar_size(&self) -> Size<i32, Physical> {
        let window_w = self
            .window_size
            .to_f64()
            .to_physical(self.scale.fractional_scale())
            .to_i32_round::<i32>()
            .w;
        Size::new(
            self.metrics.frame_left_w + window_w + self.metrics.frame_right_w,
            self.metrics.visible_top_h,
        )
    }

    fn recalculate_layout(&mut self) -> DirtyFlags {
        profiling::scope!("WindowDecorations::recalculate_layout");
        if self.window_size.w <= 0 || self.window_size.h <= 0 {
            return DirtyFlags::empty();
        }

        self.metrics = self.frame_metrics(self.bg_state());

        let old_titlebar_size = self.last_titlebar_size;
        self.last_titlebar_size = self.primary_titlebar_size();

        if !self.shadow_active() {
            self.render_state.shadow_cache.clear();
        }
        self.scaled.borrow_mut().clear();

        let mut flags = DirtyFlags::empty();
        if self.last_titlebar_size != old_titlebar_size {
            flags |= DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT;
        }
        flags
    }

    // The scale-free part of the layout: border widths, corner sizes, titlebar height and top clip,
    // for a given background state.  Computed straight from theme texture sizes + config + the
    // maximize/shade flags, so it serves both the per-scale `build_layout` and the window's logical
    // extents reported to the Space without picking a scale.
    fn frame_metrics(&self, bg_state: DecorBackgroundState) -> FrameMetrics {
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

        FrameMetrics {
            is_shaded,
            borderless_maximize,
            top_clip,
            frame_top_h,
            visible_top_h,
            frame_left_w,
            frame_right_w,
            frame_bottom_h,
            corner_top_left_size,
            corner_top_right_size,
            corner_bottom_left_size,
            corner_bottom_right_size,
        }
    }

    // Builds a decoration layout in physical pixels for `scale`.  Borders, corners and titlebar
    // height are native theme-bitmap px (scale-independent); only the content span
    // (`window_size × scale`) and hence the titlebar width depend on `scale`.  Built per output
    // into its `ScaledRender` entry, each at that output's render scale.
    fn build_layout(&self, scale: f64, title_text_size: Size<i32, Physical>) -> DecorationLayout {
        let mut layout = DecorationLayout::zeroed();
        if self.window_size.w <= 0 || self.window_size.h <= 0 {
            return layout;
        }

        let bg_state = self.bg_state();
        let frame_border_top = self.config.frame_border_top();
        let FrameMetrics {
            is_shaded,
            borderless_maximize,
            top_clip,
            frame_top_h,
            visible_top_h,
            frame_left_w,
            frame_right_w,
            frame_bottom_h,
            corner_top_left_size,
            corner_top_right_size,
            corner_bottom_left_size,
            corner_bottom_right_size,
        } = self.frame_metrics(bg_state);
        layout.top_clip = top_clip;

        let title_src_offset = Point::<i32, Buffer>::from((0, top_clip));
        let mut pieces = Vec::<PlacedPiece>::new();
        let mut push_piece =
            |role: PieceRole, placement: Rectangle<i32, Physical>, src_offset: Point<i32, Buffer>, section: FrameSection| {
                if !placement.is_empty() {
                    pieces.push(PlacedPiece {
                        role,
                        placement,
                        src_offset,
                        section,
                    });
                }
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
        push_piece(
            PieceRole::Corner(Corner::TopLeft),
            layout.top_left,
            Point::default(),
            FrameSection::Titlebar,
        );
        push_piece(
            PieceRole::Corner(Corner::TopRight),
            layout.top_right,
            Point::default(),
            FrameSection::Titlebar,
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
        push_piece(
            PieceRole::BottomSlice(BottomSlot::Left),
            layout.bottom_left,
            Point::default(),
            FrameSection::Bottom,
        );
        push_piece(
            PieceRole::BottomSlice(BottomSlot::Middle),
            layout.bottom,
            Point::default(),
            FrameSection::Bottom,
        );
        push_piece(
            PieceRole::BottomSlice(BottomSlot::Right),
            layout.bottom_right,
            Point::default(),
            FrameSection::Bottom,
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
        push_piece(PieceRole::Side(Side::Left), layout.left, Point::default(), FrameSection::Side);
        push_piece(PieceRole::Side(Side::Right), layout.right, Point::default(), FrameSection::Side);

        let btn_offset = if self.button_toggled_states.contains(ButtonToggledStates::Maximize) && self.config.borderless_maximize() {
            self.config.maximized_offset()
        } else {
            self.config.button_offset()
        };
        let btn_spacing = self.config.button_spacing();

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
                    push_piece(PieceRole::Button(*btn), extents, Point::default(), FrameSection::Titlebar);
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
                    push_piece(PieceRole::Button(*btn), extents, Point::default(), FrameSection::Titlebar);
                }
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

            let title_x;
            match &title_bg_textures {
                DecorTitleTextures::TitleStretched(_) => {
                    push_piece(
                        PieceRole::TitlePart(TitleSlot::Stretched),
                        Rectangle::new((corner_top_left_size.w + x, 0).into(), (frame_top_size.w, visible_top_h).into()),
                        title_src_offset,
                        FrameSection::Titlebar,
                    );

                    title_x = hoffset + w1 + w2;
                    let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                    layout.title_text_max_width = title_max_width;
                    layout.title_text = Rectangle::new(
                        (corner_top_left_size.w + title_x, title_y).into(),
                        (btn_right - w4, visible_top_h).into(),
                    );
                }

                DecorTitleTextures::Title5Part {
                    top1,
                    top2,
                    top3,
                    top4,
                    top5,
                    ..
                } => {
                    let visible_top_height = (top_height - top_clip).max(0);
                    let title_part = |slot, x: i32, w: i32| {
                        (
                            PieceRole::TitlePart(slot),
                            Rectangle::new((corner_top_left_size.w + x, 0).into(), (w, visible_top_h).into()),
                        )
                    };
                    let top_part = |slot, x: i32, w: i32| {
                        (
                            PieceRole::TitlePart(slot),
                            Rectangle::new((corner_top_left_size.w + x, 0).into(), (w, visible_top_height).into()),
                        )
                    };

                    if w1 > 0 {
                        let (role, rect) = title_part(TitleSlot::Title1, x, w1);
                        push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        if top1.is_some() {
                            let (role, rect) = top_part(TitleSlot::Top1, x, w1);
                            push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        }
                        x += w1;
                    }

                    let (role, rect) = title_part(TitleSlot::Title2, x, w2);
                    push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                    if top2.is_some() {
                        let (role, rect) = top_part(TitleSlot::Top2, x, w2);
                        push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                    }
                    x += w2;

                    layout.title_text = if w3 > 0 {
                        let (role, rect) = title_part(TitleSlot::Title3, x, w3);
                        push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        if top3.is_some() {
                            let (role, rect) = top_part(TitleSlot::Top3, x, w3);
                            push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        }
                        title_x = hoffset + x;
                        x += w3;

                        let title_max_width = (btn_right - w4 - title_x - self.config.title_horizontal_offset()).max(0);
                        layout.title_text_max_width = title_max_width;

                        Rectangle::new(
                            (corner_top_left_size.w + title_x, title_y).into(),
                            (btn_right - w4, visible_top_h).into(),
                        )
                    } else {
                        layout.title_text_max_width = 0;
                        Rectangle::zero()
                    };

                    x = x.min(btn_right - w4);
                    let (role, rect) = title_part(TitleSlot::Title4, x, w4);
                    push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                    if top4.is_some() {
                        let (role, rect) = top_part(TitleSlot::Top4, x, w4);
                        push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                    }
                    x += w4;

                    // Compute the remaining width after all title parts, capped at the right
                    // edge of the frame top.  xfwm4 passes the full frame width to
                    // frameFillTitlePixmap() for title5 and relies on window clipping; we have
                    // to do the arithmetic explicitly.
                    let w5_remaining = (frame_top_size.w - x).max(0);
                    if w5_remaining > 0 {
                        let (role, rect) = title_part(TitleSlot::Title5, x, w5_remaining);
                        push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        if top5.is_some() {
                            let (role, rect) = top_part(TitleSlot::Top5, x, w5_remaining);
                            push_piece(role, rect, title_src_offset, FrameSection::Titlebar);
                        }
                    }
                }
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

        layout.pieces = pieces;

        layout
    }

    // The menu button's placement at the primary scale, or empty when no menu button is shown or it
    // does not fit.  Gates the (potentially slow) window-icon lookup: the icon is only loaded when
    // the button it sits on is actually placed.  Uses the menu button's native size as the icon
    // raster size; the per-scale centred position within the button is done later by `icon_extents_for`.
    fn primary_menu_extents(&self) -> Rectangle<i32, Physical> {
        if self.shows_menu_icon() {
            self.build_layout(self.scale.fractional_scale(), Size::default())
                .pieces
                .iter()
                .find_map(|piece| matches!(piece.role, PieceRole::Button(TitlebarButton::Menu)).then_some(piece.placement))
                .unwrap_or_default()
        } else {
            Rectangle::zero()
        }
    }

    fn invalidate_render_state(&mut self, flags: DirtyFlags) {
        if flags.contains(DirtyFlags::TITLE_TEXT) {
            self.scaled.borrow_mut().clear();
        } else if flags.contains(DirtyFlags::TITLEBAR) {
            for entry in self.scaled.borrow().values() {
                entry.titlebar_texture.replace(None);
            }
        }

        if flags.intersects(DirtyFlags::TITLEBAR | DirtyFlags::TITLE_TEXT) {
            if self.render_state.window_icon_pixels.is_none() {
                let menu_extents = self.primary_menu_extents();
                self.render_state
                    .load_window_icon(&menu_extents, self.window_icon.as_ref(), &self.icon_theme);
            }
            self.render_state.invalidate_titlebar();
        }
    }

    // The opaque pixels a piece contributes to the input region, in its texture's native px, plus
    // how the renderer maps them onto the placement.  Buttons use the resting (non-hover/press)
    // bitmap so the region is stable across hover; the bottom slices come from the composited strip.
    fn piece_input_source<'a>(&'a self, role: PieceRole, bottom: &BottomTexture<'a>) -> Option<PieceInputSource<'a>> {
        let bg_state = self.bg_state();
        let from_texture = |texture: &'a DecorTexture| PieceInputSource {
            opaque_regions: texture.opaque_regions(),
            tex_size: texture.size(),
            mode: texture.rendering_mode(),
        };
        match role {
            PieceRole::Corner(Corner::TopLeft) => Some(from_texture(
                self.decoration_theme.background_texture(DecorBackgroundName::TopLeft, bg_state),
            )),
            PieceRole::Corner(Corner::TopRight) => Some(from_texture(
                self.decoration_theme.background_texture(DecorBackgroundName::TopRight, bg_state),
            )),
            PieceRole::Side(Side::Left) => Some(from_texture(
                self.decoration_theme.background_texture(DecorBackgroundName::Left, bg_state),
            )),
            PieceRole::Side(Side::Right) => Some(from_texture(
                self.decoration_theme.background_texture(DecorBackgroundName::Right, bg_state),
            )),
            PieceRole::TitlePart(slot) => {
                title_slot_texture(self.decoration_theme.title_background_textures(bg_state), slot).map(from_texture)
            }
            PieceRole::Button(button) => {
                let btn_name = DecorButtonName::from((button, self.button_toggled_states));
                self.decoration_theme
                    .button_texture(btn_name, resting_button_state(bg_state), bg_state)
                    .map(from_texture)
            }
            PieceRole::BottomSlice(slot) => {
                let (opaque_regions, tex_size, mode) = match slot {
                    BottomSlot::Left => (bottom.bottom_left_opaque, bottom.bottom_left_extents.size, DecorRenderingMode::AsIs),
                    BottomSlot::Middle => (
                        bottom.bottom_opaque,
                        bottom.bottom_extents.size,
                        DecorRenderingMode::Tiled(Direction::Horizontal),
                    ),
                    BottomSlot::Right => (
                        bottom.bottom_right_opaque,
                        bottom.bottom_right_extents.size,
                        DecorRenderingMode::AsIs,
                    ),
                };
                Some(PieceInputSource {
                    opaque_regions,
                    tex_size,
                    mode,
                })
            }
        }
    }

    // The decoration input region: the union, in frame-physical px, of every piece's opaque pixels,
    // coalesced via pixman.  Mirrors xfwm4's XSHAPE frame shape, so clicks in transparent
    // titlebar/border gaps fall through.
    fn build_input_region(&self, layout: &DecorationLayout) -> Vec<Rectangle<i32, Physical>> {
        let bottom = self.decoration_theme.bottom_background_texture(self.bg_state());
        let mut boxes = Vec::<pixman::Box32>::new();
        for piece in &layout.pieces {
            if let Some(source) = self.piece_input_source(piece.role, &bottom) {
                append_placed_region(
                    &mut boxes,
                    source.opaque_regions,
                    source.tex_size,
                    source.mode,
                    piece.placement,
                    piece.src_offset,
                );
            }
        }

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
        // The titlebar's slice of the input region, in the titlebar texture's buffer space (frame
        // coords minus the titlebar origin -- (0,0) today, but don't bake that in), eroded so that
        // after smithay grows opaque regions outward when rounding to physical at fractional scale,
        // the result stays inside the truly-opaque pixels (otherwise the rounded corners' stair-steps
        // jut past the corner).  The growth is bounded by `scale + 1` px.
        let titlebar_clipped = input_region
            .iter()
            .filter_map(|rect| rect.intersection(layout.titlebar))
            .map(|rect| {
                let loc = rect.loc - layout.titlebar.loc;
                Rectangle::<i32, Buffer>::new((loc.x, loc.y).into(), (rect.size.w, rect.size.h).into())
            })
            .collect::<Vec<_>>();
        let titlebar_opaque_region = erode_opaque_regions(&titlebar_clipped, scale.ceil() as i32 + 1);
        let title_text_pixels = render_title_text_pixels(
            pango_layout,
            title_extents,
            layout.title_text_max_width as f64,
            &self.config,
            self.bg_state(),
        );
        let menu_extents = layout
            .pieces
            .iter()
            .find_map(|piece| matches!(piece.role, PieceRole::Button(TitlebarButton::Menu)).then_some(piece.placement))
            .unwrap_or_default();
        let window_icon_extents = icon_extents_for(&menu_extents, self.render_state.window_icon_pixels.as_ref());
        ScaledRender {
            layout,
            input_region,
            titlebar_opaque_region,
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
                    // The texture is as tall as the tallest corner; drawing it to its full height
                    // (rather than the title height) lets a corner tab overhang down over the side
                    // borders.  With no overhang the texture height equals `content_top`, so this is
                    // unchanged for ordinary themes.
                    let (titlebar_location, render_size) = place_piece(rel((0, 0), (outer_right, tex.size().h)), location, scale);
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
                        Some(entry.titlebar_opaque_region.clone()),
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

// Morphological erosion by `amount` px (4-neighbour): keeps only pixels whose four axis neighbours
// are also in the region, so it removes the 1px stair-step rows of a rounded corner and insets solid
// edges.  Used to keep an opaque region inside the truly-opaque pixels after smithay grows it outward
// (`to_i32_up`/`to_physical_precise_up`) when rounding to physical at fractional scale -- under-claiming
// an opaque region only costs occlusion, never correctness.
fn erode_region(region: pixman::Region32, amount: i32) -> pixman::Region32 {
    (0..amount.max(0)).fold(region, |region, _| {
        let shift = |dx: i32, dy: i32| {
            let mut shifted = region.clone();
            shifted.translate(dx, dy);
            shifted
        };
        region
            .intersect(&shift(1, 0))
            .intersect(&shift(-1, 0))
            .intersect(&shift(0, 1))
            .intersect(&shift(0, -1))
    })
}

// Erodes buffer-px opaque rects by `amount` (coalescing through pixman) for use as a render
// element's `opaque_regions`, keeping them inside the truly-opaque pixels after smithay's outward
// rounding to physical.  See `erode_region`.
fn erode_opaque_regions(regions: &[Rectangle<i32, Buffer>], amount: i32) -> Vec<Rectangle<i32, Buffer>> {
    let boxes = regions
        .iter()
        .map(|r| pixman::Box32 {
            x1: r.loc.x,
            y1: r.loc.y,
            x2: r.loc.x + r.size.w,
            y2: r.loc.y + r.size.h,
        })
        .collect::<Vec<_>>();
    erode_region(pixman::Region32::init_rects(&boxes), amount)
        .rectangles()
        .iter()
        .map(|b| Rectangle::new((b.x1, b.y1).into(), (b.x2 - b.x1, b.y2 - b.y1).into()))
        .collect()
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
                Some(erode_opaque_regions(texture.opaque_regions(), scale.x.ceil() as i32 + 1)),
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
                Some(erode_opaque_regions(texture.opaque_regions(), scale.x.ceil() as i32 + 1)),
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
    opaque_regions: Option<Vec<Rectangle<i32, Buffer>>>,
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
        opaque_regions,
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
    // No opaque region: the inner element maps opaque regions with a linear src->geometry stretch, but
    // this element tiles (and 3-slices, for the bottom strip), so a buffer-space region would not match
    // the drawn pixels unless the texture is fully opaque.
    let element = create_texture_elem(
        context_id,
        id,
        texture,
        location,
        render_size,
        buffer_scale,
        alpha,
        src_offset,
        None,
    );

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

pub(in crate::core) fn title_slot_texture<'a>(textures: DecorTitleTextures<'a>, slot: TitleSlot) -> Option<&'a DecorTexture> {
    match (textures, slot) {
        (DecorTitleTextures::TitleStretched(tex), TitleSlot::Stretched) => Some(tex),
        (DecorTitleTextures::Title5Part { title1, .. }, TitleSlot::Title1) => Some(title1),
        (DecorTitleTextures::Title5Part { title2, .. }, TitleSlot::Title2) => Some(title2),
        (DecorTitleTextures::Title5Part { title3, .. }, TitleSlot::Title3) => Some(title3),
        (DecorTitleTextures::Title5Part { title4, .. }, TitleSlot::Title4) => Some(title4),
        (DecorTitleTextures::Title5Part { title5, .. }, TitleSlot::Title5) => Some(title5),
        (DecorTitleTextures::Title5Part { top1, .. }, TitleSlot::Top1) => top1,
        (DecorTitleTextures::Title5Part { top2, .. }, TitleSlot::Top2) => top2,
        (DecorTitleTextures::Title5Part { top3, .. }, TitleSlot::Top3) => top3,
        (DecorTitleTextures::Title5Part { top4, .. }, TitleSlot::Top4) => top4,
        (DecorTitleTextures::Title5Part { top5, .. }, TitleSlot::Top5) => top5,
        _ => None,
    }
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

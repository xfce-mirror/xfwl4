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
            Bind, Frame, ImportMem, Offscreen, Renderer, Texture,
            element::Id,
            gles::{GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformValue},
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Size, Transform},
};

use crate::core::{
    config::{TitleShadow, Xfwl4Config},
    drawing::{
        decorations::{
            DecorBackgroundName, DecorBackgroundState, DecorButtonName, DecorButtonState, DecorRenderingMode, DecorTexture,
            DecorationTheme, Direction,
        },
        shadows::{ShadowCache, ShadowKey, ShadowTexture},
    },
    shell::ssd::{ButtonToggledStates, Corner, FrameSection, HoverState, PieceRole, PressedState, title_slot_texture},
    util::{
        ImageData,
        icon_theme::{FreedesktopIconsIconTheme, IconTheme},
    },
};

#[derive(Debug, Clone)]
pub(in crate::core) struct PixelBuffer {
    pub data: Vec<u8>,
    pub size: Size<i32, Buffer>,
    pub format: Fourcc,
}

pub(in crate::core) struct DecorationRenderState {
    pub shadow_cache: ShadowCache,
    pub window_icon_pixels: Option<PixelBuffer>,
    pub titlebar_id: Id,
    pub bottom_id: Id,
    pub left_id: Id,
    pub right_id: Id,
}

impl std::fmt::Debug for DecorationRenderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecorationRenderState")
            .field("window_icon_pixels", &self.window_icon_pixels)
            .finish_non_exhaustive()
    }
}

impl DecorationRenderState {
    pub(in crate::core) fn new() -> Self {
        Self {
            shadow_cache: ShadowCache::new(),
            window_icon_pixels: None,
            titlebar_id: Id::new(),
            bottom_id: Id::new(),
            left_id: Id::new(),
            right_id: Id::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::core) fn composite_titlebar(
        renderer: &mut GlesRenderer,
        bg_state: DecorBackgroundState,
        tiling_shader: &GlesTexProgram,
        layout: &super::super::shell::ssd::DecorationLayout,
        decoration_theme: &DecorationTheme,
        button_toggled_states: ButtonToggledStates,
        hover_state: HoverState,
        pressed_state: PressedState,
        title_text_pixels: Option<&PixelBuffer>,
        window_icon_pixels: Option<&PixelBuffer>,
        window_icon_extents: Rectangle<i32, Physical>,
    ) -> anyhow::Result<Option<GlesTexture>> {
        profiling::scope!("DecorationRenderState::composite_titlebar");

        let tb_size = layout.titlebar.size;
        if tb_size.w > 0 && tb_size.h > 0 {
            let text_tex = {
                profiling::scope!("import_title_text_texture");
                title_text_pixels.and_then(|p| {
                    renderer
                        .import_memory(&p.data, p.format, p.size, false)
                        .inspect_err(|err| tracing::warn!("Failed to import title text texture: {err}"))
                        .ok()
                })
            };
            let icon_tex = {
                profiling::scope!("import_window_icon_texture");
                window_icon_pixels.and_then(|p| {
                    renderer
                        .import_memory(&p.data, p.format, p.size, false)
                        .inspect_err(|err| tracing::warn!("Failed to import window icon texture: {err}"))
                        .ok()
                })
            };

            // The layout is in physical pixels, so the titlebar is composited at native
            // resolution: theme bitmaps and the (already physical) title text are drawn 1:1 and
            // never resampled.
            //
            // A corner can be taller than the title strip (a tab overhanging below it), so size the
            // texture to the tallest piece rather than the title height -- otherwise the overhang is
            // clipped.  It is drawn by extending the titlebar quad down over the side borders in
            // render_elements.
            let tex_h = tb_size.h.max(layout.top_left.size.h).max(layout.top_right.size.h);
            let buffer_size = Size::<i32, Buffer>::new(tb_size.w, tex_h);
            let physical_size = Size::<i32, Physical>::new(tb_size.w, tex_h);

            let mut offscreen: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
            let mut fb = renderer.bind(&mut offscreen)?;
            let mut frame = renderer.render(&mut fb, physical_size, Transform::Normal)?;

            frame.clear([0., 0., 0., 0.].into(), &[Rectangle::from_size(physical_size)])?;

            {
                profiling::scope!("draw_titlebar_background");
                for piece in layout.pieces.iter().filter(|piece| piece.section == FrameSection::Titlebar) {
                    let piece_src_offset = (piece.src_offset != Point::default()).then_some(piece.src_offset);
                    match piece.role {
                        PieceRole::Corner(Corner::TopLeft) => draw_decor_texture(
                            &mut frame,
                            decoration_theme.background_texture(DecorBackgroundName::TopLeft, bg_state),
                            piece.placement,
                            None,
                            tiling_shader,
                        )?,
                        PieceRole::Corner(Corner::TopRight) => draw_decor_texture(
                            &mut frame,
                            decoration_theme.background_texture(DecorBackgroundName::TopRight, bg_state),
                            piece.placement,
                            None,
                            tiling_shader,
                        )?,
                        PieceRole::TitlePart(slot) => {
                            if let Some(texture) = title_slot_texture(decoration_theme.title_background_textures(bg_state), slot) {
                                draw_decor_texture(&mut frame, texture, piece.placement, piece_src_offset, tiling_shader)?;
                            }
                        }
                        _ => (),
                    }
                }
            }

            {
                profiling::scope!("draw_title_text_buttons_icon");
                if let Some(tex) = &text_tex
                    && !layout.title_text.is_empty()
                    && let Some(pixels) = title_text_pixels
                {
                    let text_size = Size::<i32, Physical>::new(pixels.size.w, pixels.size.h);
                    let text_extents = Rectangle::new(layout.title_text.loc, text_size);
                    draw_texture(&mut frame, tex, text_extents, None, None)?;
                }

                for piece in layout.pieces.iter().filter(|piece| piece.section == FrameSection::Titlebar) {
                    if let PieceRole::Button(button) = piece.role {
                        let btn_name = DecorButtonName::from((button, button_toggled_states));
                        let btn_state = DecorButtonState::from((button, bg_state, hover_state, pressed_state));
                        if let Some(texture) = decoration_theme.button_texture(btn_name, btn_state, bg_state) {
                            draw_decor_texture(&mut frame, texture, piece.placement, None, tiling_shader)?;
                        }
                    }
                }

                if let Some(tex) = &icon_tex
                    && !window_icon_extents.is_empty()
                {
                    draw_texture(&mut frame, tex, window_icon_extents, None, None)?;
                }
            }

            {
                profiling::scope!("frame_finish_and_sync");
                let sync = frame.finish()?;
                renderer.wait(&sync)?;
                drop(fb);
            }

            Ok(Some(offscreen))
        } else {
            Ok(None)
        }
    }

    pub(in crate::core) fn ensure_shadow_texture(
        &self,
        renderer: &mut GlesRenderer,
        config: &Xfwl4Config,
        shadow_frame_size: Size<i32, Physical>,
    ) {
        let key = ShadowKey::from_config(config, shadow_frame_size);
        if self.shadow_cache.get(key).is_none() {
            if let Some(shadow_tex) = ShadowTexture::render(renderer, key) {
                self.shadow_cache.set(shadow_tex);
            } else {
                self.shadow_cache.clear();
            }
        }
    }

    pub(in crate::core) fn load_window_icon(
        &mut self,
        menu_extents: &Rectangle<i32, Physical>,
        window_icon: Option<&ImageData>,
        icon_theme: &FreedesktopIconsIconTheme,
    ) {
        if !menu_extents.is_empty() && self.window_icon_pixels.is_none() {
            profiling::scope!("load_window_icon");
            let pixbuf = window_icon
                .and_then(|window_icon| window_icon.load(menu_extents.size.w as u32, menu_extents.size.h as u32, 1.0, icon_theme))
                .or_else(|| {
                    icon_theme
                        .load_icon("xfwm4-default", menu_extents.size.w.min(menu_extents.size.h), 1.0)
                        .ok()
                });
            self.window_icon_pixels = pixbuf.as_ref().and_then(pixbuf_to_pixels);
        } else if menu_extents.is_empty() {
            self.window_icon_pixels = None;
        }
    }

    pub(in crate::core) fn invalidate_titlebar(&mut self) {
        self.titlebar_id = Id::new();
    }
}

// The window icon raster is native px (scale-invariant), but its position within the titlebar
// follows the menu button, which moves with the per-output titlebar width -- so the centred
// extents are computed per render scale from that scale's menu rect.
pub(in crate::core) fn icon_extents_for(
    menu_extents: &Rectangle<i32, Physical>,
    icon_pixels: Option<&PixelBuffer>,
) -> Rectangle<i32, Physical> {
    match icon_pixels {
        Some(pixels) if !menu_extents.is_empty() => {
            let icon_size: Size<i32, Physical> = (pixels.size.w, pixels.size.h).into();
            let xoff = (menu_extents.size.w - icon_size.w) / 2;
            let yoff = (menu_extents.size.h - icon_size.h) / 2;
            Rectangle::new((menu_extents.loc.x + xoff, menu_extents.loc.y + yoff).into(), icon_size)
        }
        _ => Rectangle::zero(),
    }
}

pub(in crate::core) fn create_title_layout(
    font_map: &pango::FontMap,
    font_options: &cairo::FontOptions,
    window_title: Option<&str>,
    title_font: &str,
    scale: f64,
) -> (pango::Layout, Rectangle<i32, Physical>) {
    profiling::scope!("pango_title_layout");
    let ctx = font_map.create_context();
    pangocairo::context_set_font_options(&ctx, Some(font_options));

    let layout = pango::Layout::new(&ctx);
    layout.set_text(window_title.unwrap_or(""));
    layout.set_font_description(Some(&pango::FontDescription::from_string(title_font)));
    layout.set_auto_dir(false);
    layout.set_attributes(Some(&{
        let list = pango::AttrList::new();
        list.insert(pango::AttrFloat::new_scale(scale));
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
}

pub(in crate::core) fn render_title_text_pixels(
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

pub(in crate::core) fn pixbuf_to_pixels(pixbuf: &gdk_pixbuf::Pixbuf) -> Option<PixelBuffer> {
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

fn draw_decor_texture(
    frame: &mut GlesFrame<'_, '_>,
    texture: &DecorTexture,
    extents: Rectangle<i32, Physical>,
    src_offset: Option<Point<i32, Buffer>>,
    tiling_shader: &GlesTexProgram,
) -> anyhow::Result<()> {
    let tiling = match texture.rendering_mode() {
        DecorRenderingMode::Tiled(direction) => Some((direction, tiling_shader)),
        _ => None,
    };
    draw_texture(frame, texture, extents, src_offset, tiling)
}

fn draw_texture(
    frame: &mut GlesFrame<'_, '_>,
    texture: &GlesTexture,
    extents: Rectangle<i32, Physical>,
    src_offset: Option<Point<i32, Buffer>>,
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

        let uniforms = tiling.as_ref().map(|(direction, _)| {
            let tile_mask = match direction {
                Direction::Horizontal => (1.0f32, 0.0f32),
                Direction::Vertical => (0.0f32, 1.0f32),
            };

            vec![
                Uniform::new("tex_size", UniformValue::_2f(tex_size.w as f32, tex_size.h as f32)),
                Uniform::new("geo_size", UniformValue::_2f(extents.size.w as f32, extents.size.h as f32)),
                Uniform::new("tile_mask", UniformValue::_2f(tile_mask.0, tile_mask.1)),
                Uniform::new("margin_left", UniformValue::_1f(0.)),
                Uniform::new("margin_right", UniformValue::_1f(0.)),
            ]
        });
        let tiling_shader = tiling.map(|(_, shader)| shader);

        let damage = [Rectangle::from_size(extents.size)];
        frame.render_texture_from_to(
            texture,
            src,
            extents,
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

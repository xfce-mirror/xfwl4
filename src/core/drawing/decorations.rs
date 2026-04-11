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

use std::{
    collections::HashMap,
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Deref,
    path::Path,
    rc::Rc,
};

use anyhow::anyhow;
use gtk::gdk_pixbuf;
use image::EncodableLayout;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Frame, ImportMem, Offscreen, Renderer, Texture,
            gles::{GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, UniformName, UniformType},
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Size, Transform},
};

use crate::core::util::xpm;

// Tiles a texture across a geometry, with optional 3-slice horizontal margins.
//
// tile_mask selects which axes tile: (1,0) = horizontal, (0,1) = vertical.
// When margin_left/margin_right are non-zero, the texture is treated as a
// 3-slice strip: left and right margins sample directly from the corresponding
// ends of the texture, while the center portion tiles horizontally.  Used for
// the bottom decoration strip (bottom-left corner | tiled bottom | bottom-right
// corner composed into a single texture).
const TILING_SHADER_SOURCE: &str = r#"
    //_DEFINES
    precision mediump float;
    varying vec2 v_coords;
    uniform sampler2D tex;
    uniform float alpha;
    uniform vec2 tex_size;   // source texture size in logical pixels
    uniform vec2 geo_size;   // destination geometry size in logical pixels
    uniform vec2 tile_mask;
    uniform float margin_left;   // left non-tiling region width (pixels)
    uniform float margin_right;  // right non-tiling region width (pixels)

    void main() {
        vec2 pixel = v_coords * geo_size;
        vec2 tiled = mod(pixel, tex_size) / tex_size;
        vec2 uv = mix(v_coords, tiled, tile_mask);

        // 3-slice: left/right margins pass through, center tiles.
        if (margin_left > 0.0 || margin_right > 0.0) {
            if (pixel.x < margin_left) {
                uv.x = pixel.x / tex_size.x;
            } else if (pixel.x >= geo_size.x - margin_right) {
                float right_offset = pixel.x - (geo_size.x - margin_right);
                uv.x = (tex_size.x - margin_right + right_offset) / tex_size.x;
            } else {
                float center_tex_w = tex_size.x - margin_left - margin_right;
                float center_offset = mod(pixel.x - margin_left, center_tex_w);
                uv.x = (margin_left + center_offset) / tex_size.x;
            }
        }

        // Decoration textures have binary alpha (0 or 255).  Bilinear filtering
        // creates semi-transparent fringes at opaque/transparent boundaries inside
        // the texture.  Threshold back to binary and recover the original color
        // via un-premultiplication (rgb/a).
        vec4 color = texture2D(tex, uv);
        if (color.a >= 0.5) {
            color = vec4(color.rgb / color.a, 1.0);
        } else {
            color = vec4(0.0);
        }
        gl_FragColor = color * alpha;
    }
"#;

const UNIFORM_NAME_TEX_SIZE: &str = "tex_size";
const UNIFORM_NAME_GEO_SIZE: &str = "geo_size";
const UNIFORM_NAME_TILE_MASK: &str = "tile_mask";
const UNIFORM_NAME_MARGIN_LEFT: &str = "margin_left";
const UNIFORM_NAME_MARGIN_RIGHT: &str = "margin_right";

trait TextureName: fmt::Display + Copy {}
trait BackgroundName: TextureName {}

trait StateName: fmt::Display + Copy {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TitleBackgroundName {
    Title,
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

impl TitleBackgroundName {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Title1 => "title-1",
            Self::Top1 => "top-1",
            Self::Title2 => "title-2",
            Self::Top2 => "top-2",
            Self::Title3 => "title-3",
            Self::Top3 => "top-3",
            Self::Title4 => "title-4",
            Self::Top4 => "top-4",
            Self::Title5 => "title-5",
            Self::Top5 => "top-5",
        }
    }
}

impl fmt::Display for TitleBackgroundName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TextureName for TitleBackgroundName {}
impl BackgroundName for TitleBackgroundName {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorBackgroundNameInternal {
    TopLeft,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl DecorBackgroundNameInternal {
    fn as_str(&self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::Left => "left",
            Self::Right => "right",
            Self::BottomLeft => "bottom-left",
            Self::Bottom => "bottom",
            Self::BottomRight => "bottom-right",
        }
    }
}

impl fmt::Display for DecorBackgroundNameInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TextureName for DecorBackgroundNameInternal {}
impl BackgroundName for DecorBackgroundNameInternal {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorBackgroundName {
    TopLeft,
    TopRight,
    Left,
    Right,
}

impl DecorBackgroundName {
    fn as_str(&self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

impl fmt::Display for DecorBackgroundName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorButtonName {
    Close,
    Hide,
    Maximize,
    MaximizeToggled,
    Menu,
    Shade,
    ShadeToggled,
    Stick,
    StickToggled,
}

impl DecorButtonName {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Hide => "hide",
            Self::Maximize => "maximize",
            Self::MaximizeToggled => "maximize-toggled",
            Self::Menu => "menu",
            Self::Shade => "shade",
            Self::ShadeToggled => "shade-toggled",
            Self::Stick => "stick",
            Self::StickToggled => "stick-toggled",
        }
    }
}

impl fmt::Display for DecorButtonName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TextureName for DecorButtonName {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorBackgroundState {
    Active,
    Inactive,
}

impl DecorBackgroundState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}

impl fmt::Display for DecorBackgroundState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl StateName for DecorBackgroundState {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorButtonState {
    Active,
    Inactive,
    Prelight,
    Pressed,
}

impl DecorButtonState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Prelight => "prelight",
            Self::Pressed => "pressed",
        }
    }
}

impl fmt::Display for DecorButtonState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl StateName for DecorButtonState {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StretchSearchMode {
    NonStretchOnly,
    StretchOnly,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecorRenderingMode {
    AsIs,
    Tiled(Direction),
    Stretched(Direction),
}

#[derive(Debug)]
pub struct DecorTexture {
    texture: GlesTexture,
    rendering_mode: DecorRenderingMode,
}

impl Deref for DecorTexture {
    type Target = GlesTexture;

    fn deref(&self) -> &Self::Target {
        &self.texture
    }
}

impl DecorTexture {
    fn new(texture: GlesTexture, rendering_mode: DecorRenderingMode) -> Self {
        Self { texture, rendering_mode }
    }

    pub fn rendering_mode(&self) -> DecorRenderingMode {
        self.rendering_mode
    }
}

#[derive(Debug)]
struct BackgroundTextures {
    active: DecorTexture,
    inactive: DecorTexture,
}

impl BackgroundTextures {
    fn texture_for_state(&self, state: DecorBackgroundState) -> &DecorTexture {
        match state {
            DecorBackgroundState::Active => &self.active,
            DecorBackgroundState::Inactive => &self.inactive,
        }
    }

    fn with_rendering_mode(mut self, mode: DecorRenderingMode) -> Self {
        self.active.rendering_mode = mode;
        self.inactive.rendering_mode = mode;
        self
    }
}

#[derive(Debug)]
struct ButtonTextures {
    active: DecorTexture,
    inactive: DecorTexture,
    prelight: Option<DecorTexture>,
    pressed: Option<DecorTexture>,
}

impl ButtonTextures {
    fn texture_for_state(&self, state: DecorButtonState, bg_state: DecorBackgroundState) -> &DecorTexture {
        let base = match bg_state {
            DecorBackgroundState::Active => &self.active,
            DecorBackgroundState::Inactive => &self.inactive,
        };
        match state {
            DecorButtonState::Active => &self.active,
            DecorButtonState::Inactive => &self.inactive,
            DecorButtonState::Prelight => self.prelight.as_ref().unwrap_or(base),
            DecorButtonState::Pressed => self.pressed.as_ref().unwrap_or(base),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum TitleTextures {
    TitleStretched(BackgroundTextures),
    Title5Part {
        title1: BackgroundTextures,
        top1: Option<BackgroundTextures>,
        title2: BackgroundTextures,
        top2: Option<BackgroundTextures>,
        title3: BackgroundTextures,
        top3: Option<BackgroundTextures>,
        title4: BackgroundTextures,
        top4: Option<BackgroundTextures>,
        title5: BackgroundTextures,
        top5: Option<BackgroundTextures>,
    },
}

#[derive(Debug)]
struct BottomTextureInternal {
    active: GlesTexture,
    active_tiled_extents: Rectangle<i32, Buffer>,
    inactive: GlesTexture,
    inactive_tiled_extents: Rectangle<i32, Buffer>,
}

impl BottomTextureInternal {
    pub fn texture_for_state(&self, state: DecorBackgroundState) -> (&GlesTexture, Rectangle<i32, Buffer>) {
        match state {
            DecorBackgroundState::Active => (&self.active, self.active_tiled_extents),
            DecorBackgroundState::Inactive => (&self.inactive, self.inactive_tiled_extents),
        }
    }
}

#[derive(Debug)]
pub struct BottomTexture<'a> {
    pub texture: &'a GlesTexture,
    pub bottom_left_extents: Rectangle<i32, Buffer>,
    pub bottom_extents: Rectangle<i32, Buffer>,
    pub bottom_right_extents: Rectangle<i32, Buffer>,
}

#[derive(Debug)]
pub struct DecorationThemeInner {
    theme_id: u64,

    tiling_shader: GlesTexProgram,

    top_left: BackgroundTextures,
    title: TitleTextures,
    top_right: BackgroundTextures,
    left: BackgroundTextures,
    right: BackgroundTextures,
    bottom_strip: BottomTextureInternal,

    close: Option<ButtonTextures>,
    hide: Option<ButtonTextures>,
    maximize: Option<ButtonTextures>,
    maximize_toggled: Option<ButtonTextures>,
    menu: Option<ButtonTextures>,
    shade: Option<ButtonTextures>,
    shade_toggled: Option<ButtonTextures>,
    stick: Option<ButtonTextures>,
    stick_toggled: Option<ButtonTextures>,
}

#[derive(Debug, Clone)]
pub struct DecorationTheme {
    inner: Rc<DecorationThemeInner>,
}

pub enum DecorTitleTextures<'a> {
    TitleStretched(&'a DecorTexture),
    Title5Part {
        title1: &'a DecorTexture,
        top1: Option<&'a DecorTexture>,
        title2: &'a DecorTexture,
        top2: Option<&'a DecorTexture>,
        title3: &'a DecorTexture,
        top3: Option<&'a DecorTexture>,
        title4: &'a DecorTexture,
        top4: Option<&'a DecorTexture>,
        title5: &'a DecorTexture,
        top5: Option<&'a DecorTexture>,
    },
}

impl DecorationTheme {
    pub fn load<P: AsRef<Path>, R: AsMut<GlesRenderer>>(
        mut renderer: R,
        theme_path: P,

        theme_colors: &HashMap<String, [u16; 4]>,
    ) -> anyhow::Result<Self> {
        let renderer = renderer.as_mut();
        let theme_path = theme_path.as_ref();

        Ok(DecorationTheme {
            inner: Rc::new(DecorationThemeInner {
                theme_id: {
                    let mut hasher = DefaultHasher::new();
                    theme_path.to_string_lossy().hash(&mut hasher);
                    hasher.finish()
                },
                // XXX: technically we only need to compile this once, not on every theme change
                tiling_shader: renderer.compile_custom_texture_shader(
                    TILING_SHADER_SOURCE,
                    &[
                        UniformName::new(UNIFORM_NAME_TEX_SIZE, UniformType::_2f),
                        UniformName::new(UNIFORM_NAME_GEO_SIZE, UniformType::_2f),
                        UniformName::new(UNIFORM_NAME_TILE_MASK, UniformType::_2f),
                        UniformName::new(UNIFORM_NAME_MARGIN_LEFT, UniformType::_1f),
                        UniformName::new(UNIFORM_NAME_MARGIN_RIGHT, UniformType::_1f),
                    ],
                )?,
                top_left: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundNameInternal::TopLeft,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                    theme_colors,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),
                title: load_title_textures(renderer, theme_path, theme_colors)?,
                top_right: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundNameInternal::TopRight,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                    theme_colors,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),
                left: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundNameInternal::Left,
                    StretchSearchMode::Both,
                    Direction::Vertical,
                    theme_colors,
                )?,
                right: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundNameInternal::Right,
                    StretchSearchMode::Both,
                    Direction::Vertical,
                    theme_colors,
                )?,
                bottom_strip: load_bottom_textures(renderer, theme_path, theme_colors)?,

                close: load_button_texture(renderer, theme_path, DecorButtonName::Close, theme_colors).ok(),
                hide: load_button_texture(renderer, theme_path, DecorButtonName::Hide, theme_colors).ok(),
                maximize: load_button_texture(renderer, theme_path, DecorButtonName::Maximize, theme_colors).ok(),
                maximize_toggled: load_button_texture(renderer, theme_path, DecorButtonName::MaximizeToggled, theme_colors).ok(),
                menu: load_button_texture(renderer, theme_path, DecorButtonName::Menu, theme_colors).ok(),
                shade: load_button_texture(renderer, theme_path, DecorButtonName::Shade, theme_colors).ok(),
                shade_toggled: load_button_texture(renderer, theme_path, DecorButtonName::ShadeToggled, theme_colors).ok(),
                stick: load_button_texture(renderer, theme_path, DecorButtonName::Stick, theme_colors).ok(),
                stick_toggled: load_button_texture(renderer, theme_path, DecorButtonName::StickToggled, theme_colors).ok(),
            }),
        })
    }

    pub fn theme_id(&self) -> u64 {
        self.inner.theme_id
    }

    pub fn tiling_shader(&self) -> &GlesTexProgram {
        &self.inner.tiling_shader
    }

    pub fn title_background_textures<'a>(&'a self, state: DecorBackgroundState) -> DecorTitleTextures<'a> {
        match &self.inner.title {
            TitleTextures::TitleStretched(textures) => DecorTitleTextures::TitleStretched(textures.texture_for_state(state)),
            TitleTextures::Title5Part {
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
            } => DecorTitleTextures::Title5Part {
                title1: title1.texture_for_state(state),
                top1: top1.as_ref().map(|textures| textures.texture_for_state(state)),
                title2: title2.texture_for_state(state),
                top2: top2.as_ref().map(|textures| textures.texture_for_state(state)),
                title3: title3.texture_for_state(state),
                top3: top3.as_ref().map(|textures| textures.texture_for_state(state)),
                title4: title4.texture_for_state(state),
                top4: top4.as_ref().map(|textures| textures.texture_for_state(state)),
                title5: title5.texture_for_state(state),
                top5: top5.as_ref().map(|textures| textures.texture_for_state(state)),
            },
        }
    }

    pub fn bottom_background_texture<'a>(&'a self, state: DecorBackgroundState) -> BottomTexture<'a> {
        let (texture, bottom_extents) = self.inner.bottom_strip.texture_for_state(state);
        BottomTexture {
            texture,
            bottom_left_extents: Rectangle::new((0, 0).into(), (bottom_extents.loc.x, texture.size().h).into()),
            bottom_extents,
            bottom_right_extents: Rectangle::new(
                (bottom_extents.loc.x + bottom_extents.size.w, 0).into(),
                (texture.size().w - bottom_extents.size.w - bottom_extents.loc.x, texture.size().h).into(),
            ),
        }
    }

    pub fn background_texture(&self, name: DecorBackgroundName, state: DecorBackgroundState) -> &DecorTexture {
        match name {
            DecorBackgroundName::Left => self.inner.left.texture_for_state(state),
            DecorBackgroundName::Right => self.inner.right.texture_for_state(state),
            DecorBackgroundName::TopLeft => self.inner.top_left.texture_for_state(state),
            DecorBackgroundName::TopRight => self.inner.top_right.texture_for_state(state),
        }
    }

    pub fn button_texture(&self, name: DecorButtonName, state: DecorButtonState, bg_state: DecorBackgroundState) -> Option<&DecorTexture> {
        match name {
            DecorButtonName::Hide => self.inner.hide.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::Close => self.inner.close.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::Maximize => self.inner.maximize.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::MaximizeToggled => self
                .inner
                .maximize_toggled
                .as_ref()
                .or_else(|| self.inner.maximize.as_ref())
                .map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::Menu => self.inner.menu.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::Shade => self.inner.shade.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::ShadeToggled => self
                .inner
                .shade_toggled
                .as_ref()
                .or_else(|| self.inner.shade.as_ref())
                .map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::Stick => self.inner.stick.as_ref().map(|t| t.texture_for_state(state, bg_state)),
            DecorButtonName::StickToggled => self
                .inner
                .stick_toggled
                .as_ref()
                .or_else(|| self.inner.stick.as_ref())
                .map(|t| t.texture_for_state(state, bg_state)),
        }
    }
}

fn load_background_texture<P: AsRef<Path>, N: BackgroundName>(
    renderer: &mut GlesRenderer,
    theme_path: P,
    name: N,
    stretch_search_mode: StretchSearchMode,
    direction: Direction,
    theme_colors: &HashMap<String, [u16; 4]>,
) -> anyhow::Result<BackgroundTextures> {
    let theme_path = theme_path.as_ref();

    let (active_pix, active_mode) = load_compose_image(
        theme_path,
        name,
        DecorBackgroundState::Active,
        stretch_search_mode,
        direction,
        theme_colors,
    )?;
    let (inactive_pix, inactive_mode) = load_compose_image(
        theme_path,
        name,
        DecorBackgroundState::Inactive,
        stretch_search_mode,
        direction,
        theme_colors,
    )?;

    let active = import_texture(renderer, active_pix)?;
    let inactive = import_texture(renderer, inactive_pix)?;

    Ok(BackgroundTextures {
        active: DecorTexture::new(active, active_mode),
        inactive: DecorTexture::new(inactive, inactive_mode),
    })
}

fn load_title_textures<P: AsRef<Path>>(
    renderer: &mut GlesRenderer,
    theme_path: P,

    theme_colors: &HashMap<String, [u16; 4]>,
) -> anyhow::Result<TitleTextures> {
    let theme_path = theme_path.as_ref();

    if let Ok(title_stretch) = load_background_texture(
        renderer,
        theme_path,
        TitleBackgroundName::Title,
        StretchSearchMode::StretchOnly,
        Direction::Horizontal,
        theme_colors,
    ) {
        Ok(TitleTextures::TitleStretched(title_stretch))
    } else {
        Ok(TitleTextures::Title5Part {
            title1: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title1,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )?,
            title2: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title2,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )?,
            title3: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title3,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )?,
            title4: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title4,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )?,
            title5: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title5,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )?,
            top1: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top1,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )
            .ok(),
            top2: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top2,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )
            .ok(),
            top3: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top3,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )
            .ok(),
            top4: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top4,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )
            .ok(),
            top5: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top5,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
                theme_colors,
            )
            .ok(),
        })
    }
}

fn load_bottom_textures<P: AsRef<Path>>(
    renderer: &mut GlesRenderer,
    theme_path: P,
    theme_colors: &HashMap<String, [u16; 4]>,
) -> anyhow::Result<BottomTextureInternal> {
    let bottom_left = load_background_texture(
        renderer,
        &theme_path,
        DecorBackgroundNameInternal::BottomLeft,
        StretchSearchMode::Both,
        Direction::Horizontal,
        theme_colors,
    )?
    .with_rendering_mode(DecorRenderingMode::AsIs);
    let bottom = load_background_texture(
        renderer,
        &theme_path,
        DecorBackgroundNameInternal::Bottom,
        StretchSearchMode::Both,
        Direction::Horizontal,
        theme_colors,
    )?;
    let bottom_right = load_background_texture(
        renderer,
        &theme_path,
        DecorBackgroundNameInternal::BottomRight,
        StretchSearchMode::Both,
        Direction::Horizontal,
        theme_colors,
    )?
    .with_rendering_mode(DecorRenderingMode::AsIs);

    fn render_part(frame: &mut GlesFrame<'_, '_>, texture: &GlesTexture, dest_loc: Point<i32, Physical>) -> anyhow::Result<()> {
        let tex_size = texture.size();
        let src = Rectangle::from_size(tex_size).to_f64();
        let dest = Rectangle::new(dest_loc, (tex_size.w, tex_size.h).into());
        let damage = [Rectangle::from_size(dest.size)];
        frame.render_texture_from_to(texture, src, dest, &damage, &[], Transform::Normal, 1., None, &[])?;
        Ok(())
    }

    fn render_strip(
        renderer: &mut GlesRenderer,
        bottom_left: &BackgroundTextures,
        bottom: &BackgroundTextures,
        bottom_right: &BackgroundTextures,
        state: DecorBackgroundState,
    ) -> anyhow::Result<(GlesTexture, Rectangle<i32, Buffer>)> {
        let bottom_left_tex = bottom_left.texture_for_state(state);
        let bottom_left_tex_size = bottom_left_tex.size();
        let bottom_tex = bottom.texture_for_state(state);
        let bottom_tex_size = bottom_tex.size();
        let bottom_right_tex = bottom_right.texture_for_state(state);
        let bottom_right_tex_size = bottom_right_tex.size();

        let buffer_size = Size::<_, Buffer>::new(
            bottom_left_tex_size.w + bottom_tex_size.w + bottom_right_tex_size.w,
            bottom_left_tex_size.h.max(bottom_tex_size.h).max(bottom_right_tex_size.h),
        );
        let physical_size = (buffer_size.w, buffer_size.h).into();

        let mut offscreen: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        let mut fb = renderer.bind(&mut offscreen)?;
        let mut frame = renderer.render(&mut fb, physical_size, Transform::Normal)?;
        frame.clear([0., 0., 0., 0.].into(), &[Rectangle::from_size(physical_size)])?;

        let max_h = bottom_left_tex_size.h.max(bottom_tex_size.h).max(bottom_right_tex_size.h);

        let bottom_left_dest_loc = (0, max_h - bottom_left_tex_size.h).into();
        render_part(&mut frame, bottom_left_tex, bottom_left_dest_loc)?;

        let bottom_dest_loc = (bottom_left_tex_size.w, max_h - bottom_tex_size.h).into();
        render_part(&mut frame, bottom_tex, bottom_dest_loc)?;

        let bottom_right_dest_loc = (bottom_left_tex_size.w + bottom_tex_size.w, max_h - bottom_right_tex_size.h).into();
        render_part(&mut frame, bottom_right_tex, bottom_right_dest_loc)?;

        let sync = frame.finish()?;
        renderer.wait(&sync)?;
        drop(fb);

        Ok((offscreen, Rectangle::new((bottom_left_tex_size.w, 0).into(), bottom_tex_size)))
    }

    let (active, active_tiled_extents) = render_strip(renderer, &bottom_left, &bottom, &bottom_right, DecorBackgroundState::Active)?;
    let (inactive, inactive_tiled_extents) = render_strip(renderer, &bottom_left, &bottom, &bottom_right, DecorBackgroundState::Inactive)?;

    Ok(BottomTextureInternal {
        active,
        active_tiled_extents,
        inactive,
        inactive_tiled_extents,
    })
}

fn load_button_texture<P: AsRef<Path>>(
    renderer: &mut GlesRenderer,
    theme_path: P,
    name: DecorButtonName,
    theme_colors: &HashMap<String, [u16; 4]>,
) -> anyhow::Result<ButtonTextures> {
    let theme_path = theme_path.as_ref();

    let (active_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Active,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
        theme_colors,
    )?;
    let (inactive_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Inactive,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
        theme_colors,
    )?;
    let prelight_pix = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Prelight,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
        theme_colors,
    )
    .ok()
    .map(|(pix, _)| pix);
    let pressed_pix = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Pressed,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
        theme_colors,
    )
    .ok()
    .map(|(pix, _)| pix);

    let active = import_texture(renderer, active_pix)?;
    let inactive = import_texture(renderer, inactive_pix)?;
    let prelight = prelight_pix.map(|pix| import_texture(renderer, pix)).transpose()?;
    let pressed = pressed_pix.map(|pix| import_texture(renderer, pix)).transpose()?;

    Ok(ButtonTextures {
        active: DecorTexture {
            texture: active,
            rendering_mode: DecorRenderingMode::AsIs,
        },
        inactive: DecorTexture {
            texture: inactive,
            rendering_mode: DecorRenderingMode::AsIs,
        },
        prelight: prelight.map(|texture| DecorTexture {
            texture,
            rendering_mode: DecorRenderingMode::AsIs,
        }),
        pressed: pressed.map(|texture| DecorTexture {
            texture,
            rendering_mode: DecorRenderingMode::AsIs,
        }),
    })
}

fn import_texture(renderer: &mut GlesRenderer, pixbuf: gdk_pixbuf::Pixbuf) -> anyhow::Result<GlesTexture> {
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf
    } else {
        pixbuf
            .add_alpha(false, 0, 0, 0)
            .map_err(|err| anyhow!("Failed to add alpha channel to pixbuf: {err}"))?
    };

    // xfwm4 uses a binary alpha threshold when rendering decorations: only fully opaque pixels
    // are visible, and everything else is fully transparent.  The PNG overlays in themes contain
    // shadow effects with intermediate alpha values that are designed to be discarded this way.
    let src = pixbuf.read_pixel_bytes();
    let data: Vec<u8> = src
        .as_bytes()
        .chunks_exact(4)
        .flat_map(|p| if p[3] == 255 { [p[0], p[1], p[2], 255] } else { [0, 0, 0, 0] })
        .collect();

    Ok(renderer.import_memory(&data, Fourcc::Abgr8888, Size::new(pixbuf.width(), pixbuf.height()), false)?)
}

fn load_compose_image<P: AsRef<Path>, N: TextureName, S: StateName>(
    theme_path: P,
    name: N,
    state: S,
    stretch_search_mode: StretchSearchMode,
    direction: Direction,
    theme_colors: &HashMap<String, [u16; 4]>,
) -> anyhow::Result<(gdk_pixbuf::Pixbuf, DecorRenderingMode)> {
    const OVERLAY_IMAGE_TYPES: &[&str] = &["svg", "png", "gif", "jpg", "bmp"];

    let theme_path = theme_path.as_ref();

    let mut path = theme_path.to_path_buf();
    let (base, has_base_stretch) = if stretch_search_mode != StretchSearchMode::NonStretchOnly {
        path.push(format!("{name}-{state}-stretch.xpm"));
        load_xpm(path, theme_colors).map(|base| (base, true)).ok().unzip()
    } else {
        (None, None)
    };

    let (base, has_base_stretch) = if base.is_none() && stretch_search_mode != StretchSearchMode::StretchOnly {
        let mut path = theme_path.to_path_buf();
        path.push(format!("{name}-{state}.xpm"));
        load_xpm(path, theme_colors).map(|base| (base, false)).ok().unzip()
    } else {
        (base, has_base_stretch)
    };

    let mut iter = OVERLAY_IMAGE_TYPES.iter();
    let (overlay, has_overlay_stretch) = loop {
        if let Some(ext) = iter.next() {
            if stretch_search_mode != StretchSearchMode::NonStretchOnly && (has_base_stretch.is_none() || has_base_stretch == Some(true)) {
                let mut path = theme_path.to_path_buf();
                path.push(format!("{name}-{state}-stretch.{ext}"));

                if let Ok(pixbuf) = gdk_pixbuf::Pixbuf::from_file(path) {
                    break (Some(pixbuf), true);
                } else if has_base_stretch == Some(true) {
                    // Here we have to continue, because we have a base image that is stretched, so
                    // we don't want to try for a non-stretched overlay image.
                    continue;
                }
            }

            if stretch_search_mode != StretchSearchMode::StretchOnly {
                let mut path = theme_path.to_path_buf();
                path.push(format!("{name}-{state}.{ext}"));
                if let Ok(pixbuf) = gdk_pixbuf::Pixbuf::from_file(path) {
                    break (Some(pixbuf), false);
                }
            }
        } else {
            break (None, false);
        }
    };

    let final_image = match (base, overlay) {
        (Some(base), Some(overlay)) => {
            let width = base.width().min(overlay.width());
            let height = base.height().min(overlay.height());
            overlay.composite(&base, 0, 0, width, height, 0., 0., 1., 1., gdk_pixbuf::InterpType::Bilinear, 0xff);
            Ok(base)
        }
        (Some(base), None) => Ok(base),
        (None, Some(overlay)) => Ok(overlay),
        (None, None) => Err(anyhow!("No usable image found for {name}-{state} in theme")),
    }?;

    let mode = if has_base_stretch.unwrap_or(has_overlay_stretch) {
        DecorRenderingMode::Stretched(direction)
    } else {
        DecorRenderingMode::Tiled(direction)
    };

    Ok((final_image, mode))
}

fn load_xpm<P: AsRef<Path>>(path: P, theme_colors: &HashMap<String, [u16; 4]>) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
    use image::ImageDecoder;
    use std::{fs::File, io::BufReader};

    let reader = BufReader::new(File::open(path.as_ref())?);
    let decoder = xpm::XpmDecoder::new(reader, theme_colors.clone()).map_err(|err| anyhow!("Failed to decode XPM header: {err}"))?;
    let (width, height) = decoder.dimensions();
    let total_bytes = decoder.total_bytes().try_into().map_err(|_| anyhow!("XPM image too large"))?;
    let mut rgba16: Vec<u8> = vec![0; total_bytes];
    decoder
        .read_image(&mut rgba16)
        .map_err(|err| anyhow!("Failed to decode XPM pixels: {err}"))?;

    let rgba8: Vec<u8> = rgba16
        .chunks_exact(2)
        .map(|pair| (u16::from_ne_bytes([pair[0], pair[1]]) >> 8) as u8)
        .collect();

    let rowstride = (width as usize).checked_mul(4).ok_or_else(|| anyhow!("XPM image too large"))?;
    Ok(gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from_owned(rgba8),
        gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        width as i32,
        height as i32,
        rowstride as i32,
    ))
}

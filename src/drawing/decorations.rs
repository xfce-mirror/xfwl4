use std::{
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
            ImportMem,
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, UniformName, UniformType},
        },
    },
    utils::Size,
};
const TILING_SHADER_SOURCE: &str = r#"
    //_DEFINES
    precision mediump float;
    varying vec2 v_coords;
    uniform sampler2D tex;
    uniform float alpha;
    uniform vec2 tex_size;
    uniform vec2 geo_size;
    uniform vec2 tile_mask;

    void main() {
        vec2 tiled = mod(v_coords * geo_size, tex_size) / tex_size;
        vec2 uv = mix(v_coords, tiled, tile_mask);
        vec4 color = texture2D(tex, uv);
        gl_FragColor = color * alpha;
    }
"#;
const UNIFORM_NAME_TEX_SIZE: &str = "tex_size";
const UNIFORM_NAME_GEO_SIZE: &str = "geo_size";
const UNIFORM_NAME_TILE_MASK: &str = "tile_mask";

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
pub enum DecorBackgroundName {
    TopLeft,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl DecorBackgroundName {
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

impl fmt::Display for DecorBackgroundName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TextureName for DecorBackgroundName {}
impl BackgroundName for DecorBackgroundName {}

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
    prelight: DecorTexture,
    pressed: DecorTexture,
}

impl ButtonTextures {
    fn texture_for_state(&self, state: DecorButtonState) -> &DecorTexture {
        match state {
            DecorButtonState::Active => &self.active,
            DecorButtonState::Inactive => &self.inactive,
            DecorButtonState::Prelight => &self.prelight,
            DecorButtonState::Pressed => &self.pressed,
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
pub struct DecorationThemeInner {
    theme_id: u64,

    tiling_shader: GlesTexProgram,

    top_left: BackgroundTextures,
    title: TitleTextures,
    top_right: BackgroundTextures,
    left: BackgroundTextures,
    right: BackgroundTextures,
    bottom_left: BackgroundTextures,
    bottom: BackgroundTextures,
    bottom_right: BackgroundTextures,

    close: ButtonTextures,
    hide: ButtonTextures,
    maximize: ButtonTextures,
    maximize_toggled: ButtonTextures,
    menu: ButtonTextures,
    shade: ButtonTextures,
    shade_toggled: ButtonTextures,
    stick: ButtonTextures,
    stick_toggled: ButtonTextures,
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
    pub fn load<P: AsRef<Path>, R: AsMut<GlesRenderer>>(mut renderer: R, theme_path: P) -> anyhow::Result<Self> {
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
                    ],
                )?,
                top_left: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::TopLeft,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),
                title: load_title_textures(renderer, theme_path)?,
                top_right: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::TopRight,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),
                left: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::Left,
                    StretchSearchMode::Both,
                    Direction::Vertical,
                )?,
                right: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::Right,
                    StretchSearchMode::Both,
                    Direction::Vertical,
                )?,
                bottom_left: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::BottomLeft,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),
                bottom: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::Bottom,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                )?,
                bottom_right: load_background_texture(
                    renderer,
                    theme_path,
                    DecorBackgroundName::BottomRight,
                    StretchSearchMode::Both,
                    Direction::Horizontal,
                )?
                .with_rendering_mode(DecorRenderingMode::AsIs),

                close: load_button_texture(renderer, theme_path, DecorButtonName::Close)?,
                hide: load_button_texture(renderer, theme_path, DecorButtonName::Hide)?,
                maximize: load_button_texture(renderer, theme_path, DecorButtonName::Maximize)?,
                maximize_toggled: load_button_texture(renderer, theme_path, DecorButtonName::MaximizeToggled)?,
                menu: load_button_texture(renderer, theme_path, DecorButtonName::Menu)?,
                shade: load_button_texture(renderer, theme_path, DecorButtonName::Shade)?,
                shade_toggled: load_button_texture(renderer, theme_path, DecorButtonName::ShadeToggled)?,
                stick: load_button_texture(renderer, theme_path, DecorButtonName::Stick)?,
                stick_toggled: load_button_texture(renderer, theme_path, DecorButtonName::StickToggled)?,
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

    pub fn background_texture(&self, name: DecorBackgroundName, state: DecorBackgroundState) -> &DecorTexture {
        match name {
            DecorBackgroundName::Left => self.inner.left.texture_for_state(state),
            DecorBackgroundName::Right => self.inner.right.texture_for_state(state),
            DecorBackgroundName::Bottom => self.inner.bottom.texture_for_state(state),
            DecorBackgroundName::TopLeft => self.inner.top_left.texture_for_state(state),
            DecorBackgroundName::TopRight => self.inner.top_right.texture_for_state(state),
            DecorBackgroundName::BottomLeft => self.inner.bottom_left.texture_for_state(state),
            DecorBackgroundName::BottomRight => self.inner.bottom_right.texture_for_state(state),
        }
    }

    pub fn button_texture(&self, name: DecorButtonName, state: DecorButtonState) -> &DecorTexture {
        match name {
            DecorButtonName::Hide => self.inner.hide.texture_for_state(state),
            DecorButtonName::Close => self.inner.close.texture_for_state(state),
            DecorButtonName::Maximize => self.inner.maximize.texture_for_state(state),
            DecorButtonName::MaximizeToggled => self.inner.maximize_toggled.texture_for_state(state),
            DecorButtonName::Menu => self.inner.menu.texture_for_state(state),
            DecorButtonName::Shade => self.inner.shade.texture_for_state(state),
            DecorButtonName::ShadeToggled => self.inner.shade_toggled.texture_for_state(state),
            DecorButtonName::Stick => self.inner.stick.texture_for_state(state),
            DecorButtonName::StickToggled => self.inner.stick_toggled.texture_for_state(state),
        }
    }
}

fn load_background_texture<P: AsRef<Path>, N: BackgroundName>(
    renderer: &mut GlesRenderer,
    theme_path: P,
    name: N,
    stretch_search_mode: StretchSearchMode,
    direction: Direction,
) -> anyhow::Result<BackgroundTextures> {
    let theme_path = theme_path.as_ref();

    let (active_pix, active_mode) = load_compose_image(theme_path, name, DecorBackgroundState::Active, stretch_search_mode, direction)?;
    let (inactive_pix, inactive_mode) =
        load_compose_image(theme_path, name, DecorBackgroundState::Inactive, stretch_search_mode, direction)?;

    let active = import_texture(renderer, active_pix)?;
    let inactive = import_texture(renderer, inactive_pix)?;

    Ok(BackgroundTextures {
        active: DecorTexture::new(active, active_mode),
        inactive: DecorTexture::new(inactive, inactive_mode),
    })
}

fn load_title_textures<P: AsRef<Path>>(renderer: &mut GlesRenderer, theme_path: P) -> anyhow::Result<TitleTextures> {
    let theme_path = theme_path.as_ref();

    if let Ok(title_stretch) = load_background_texture(
        renderer,
        theme_path,
        TitleBackgroundName::Title,
        StretchSearchMode::StretchOnly,
        Direction::Horizontal,
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
            )?,
            title2: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title2,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )?,
            title3: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title3,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )?,
            title4: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title4,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )?,
            title5: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Title5,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )?,
            top1: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top1,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )
            .ok(),
            top2: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top2,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )
            .ok(),
            top3: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top3,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )
            .ok(),
            top4: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top4,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )
            .ok(),
            top5: load_background_texture(
                renderer,
                theme_path,
                TitleBackgroundName::Top5,
                StretchSearchMode::NonStretchOnly,
                Direction::Horizontal,
            )
            .ok(),
        })
    }
}

fn load_button_texture<P: AsRef<Path>>(
    renderer: &mut GlesRenderer,
    theme_path: P,
    name: DecorButtonName,
) -> anyhow::Result<ButtonTextures> {
    let theme_path = theme_path.as_ref();

    let (active_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Active,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
    )?;
    let (inactive_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Inactive,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
    )?;
    let (prelight_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Prelight,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
    )?;
    let (pressed_pix, _) = load_compose_image(
        theme_path,
        name,
        DecorButtonState::Pressed,
        StretchSearchMode::NonStretchOnly,
        Direction::Horizontal,
    )?;

    let active = import_texture(renderer, active_pix)?;
    let inactive = import_texture(renderer, inactive_pix)?;
    let prelight = import_texture(renderer, prelight_pix)?;
    let pressed = import_texture(renderer, pressed_pix)?;

    Ok(ButtonTextures {
        active: DecorTexture {
            texture: active,
            rendering_mode: DecorRenderingMode::AsIs,
        },
        inactive: DecorTexture {
            texture: inactive,
            rendering_mode: DecorRenderingMode::AsIs,
        },
        prelight: DecorTexture {
            texture: prelight,
            rendering_mode: DecorRenderingMode::AsIs,
        },
        pressed: DecorTexture {
            texture: pressed,
            rendering_mode: DecorRenderingMode::AsIs,
        },
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
) -> anyhow::Result<(gdk_pixbuf::Pixbuf, DecorRenderingMode)> {
    const OVERLAY_IMAGE_TYPES: &[&str] = &["svg", "png", "gif", "jpg", "bmp"];

    let theme_path = theme_path.as_ref();

    let mut path = theme_path.to_path_buf();

    let (base, has_base_stretch) = if stretch_search_mode != StretchSearchMode::NonStretchOnly {
        path.push(format!("{name}-{state}-stretch.xpm"));
        gdk_pixbuf::Pixbuf::from_file(path).ok().map(|base| (base, true)).unzip()
    } else {
        (None, None)
    };

    let (base, has_base_stretch) = if base.is_none() && stretch_search_mode != StretchSearchMode::StretchOnly {
        let mut path = theme_path.to_path_buf();
        path.push(format!("{name}-{state}.xpm"));
        gdk_pixbuf::Pixbuf::from_file(path).ok().map(|base| (base, false)).unzip()
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

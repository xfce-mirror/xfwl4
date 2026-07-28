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

use std::ops::RangeInclusive;

use x11rb::{
    COPY_DEPTH_FROM_PARENT,
    connection::Connection,
    image::Image,
    properties::{WmHints, WmHintsState},
    protocol::{
        Event,
        xproto::{AtomEnum, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, PropMode, Screen, Setup, WindowClass},
    },
    wrapper::ConnectionExt as _,
};

const XK_ESCAPE: u32 = 0xff1b;

const ICON_SIZE: u16 = 48;
const ICON_LAST: u16 = ICON_SIZE - 1;

const SWATCH_ROWS: RangeInclusive<u16> = 1..=10;
const DIGIT_ROWS: RangeInclusive<u16> = 12..=32;
const RAMP_ROWS: RangeInclusive<u16> = 34..=41;
const COMB_ROWS: RangeInclusive<u16> = 42..=46;

const BEVEL_TOP_LEFT: u16 = 12;
const BEVEL_TOP_RIGHT: u16 = 8;
const BEVEL_BOTTOM_RIGHT: u16 = 4;

const NOTCH_ROWS: RangeInclusive<u16> = 20..=27;
const NOTCH_LEFT: u16 = 40;

const COMB_PERIOD: u16 = 8;
const COMB_SLOT: u16 = 2;

const GLYPH_W: u16 = 5;
const GLYPH_H: u16 = 7;
const GLYPH_SCALE: u16 = 3;
const GLYPH_GAP: u16 = 3;
const GLYPH_ADVANCE: u16 = GLYPH_W * GLYPH_SCALE + GLYPH_GAP;
const GLYPH_LEFT: u16 = (ICON_SIZE - (GLYPH_ADVANCE * 2 - GLYPH_GAP)) / 2;

// Reserved for "the mask said this pixel is transparent". It appears nowhere
// else in the design, so any magenta on screen means the mask was not applied.
const MAGENTA: [u8; 4] = [0xff, 0x00, 0xff, 0xff];
const TRANSPARENT: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
const WHITE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const YELLOW: [u8; 4] = [0xff, 0xff, 0x00, 0xff];
const CYAN: [u8; 4] = [0x00, 0xff, 0xff, 0xff];
const RED: [u8; 4] = [0xff, 0x00, 0x00, 0xff];
const GREEN: [u8; 4] = [0x00, 0xff, 0x00, 0xff];
const BLUE: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const ORANGE: [u8; 4] = [0xff, 0x80, 0x00, 0xff];
const SLATE: [u8; 4] = [0x20, 0x30, 0x50, 0xff];
const BLACK: [u8; 4] = [0x00, 0x00, 0x00, 0xff];

const REFERENCE_SCALE: u16 = 4;
const PANEL_SIZE: u16 = ICON_SIZE * REFERENCE_SCALE;
const MARGIN: u16 = 12;
const PANEL_TOP: u16 = 30;
const PANEL_LEFT: [u16; 2] = [MARGIN, MARGIN * 2 + PANEL_SIZE];
const LABEL_BASELINE: i16 = 22;
const WIN_W: u16 = MARGIN * 3 + PANEL_SIZE * 2;
const WIN_H: u16 = PANEL_TOP + PANEL_SIZE + MARGIN;

const CHECKER_DARK: u8 = 0x50;
const CHECKER_LIGHT: u8 = 0x68;
const CHECKER_SIZE: u16 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconKind {
    Depth24,
    Depth32,
    Depth32NoMask,
    Depth32BadMask,
}

impl IconKind {
    fn depth(self) -> u8 {
        match self {
            Self::Depth24 => 24,
            Self::Depth32 | Self::Depth32NoMask | Self::Depth32BadMask => 32,
        }
    }

    fn has_mask(self) -> bool {
        self != Self::Depth32NoMask
    }

    /// Deliberately not `ICON_SIZE` for the mismatch case: nothing in X requires the
    /// mask to match the pixmap, so a compositor must survive being handed a mask it
    /// cannot index with the pixmap's coordinates.
    fn mask_size(self) -> u16 {
        match self {
            Self::Depth32BadMask => ICON_SIZE / 2,
            _ => ICON_SIZE,
        }
    }

    fn digits(self) -> &'static [u8; 2] {
        match self {
            Self::Depth24 => b"24",
            Self::Depth32 => b"32",
            Self::Depth32NoMask => b"3A",
            Self::Depth32BadMask => b"3X",
        }
    }

    fn wm_class(self) -> &'static [u8] {
        match self {
            Self::Depth24 => b"icon24\0Xfwl4WmHintsIconTest\0",
            Self::Depth32 => b"icon32\0Xfwl4WmHintsIconTest\0",
            Self::Depth32NoMask => b"icon32alpha\0Xfwl4WmHintsIconTest\0",
            Self::Depth32BadMask => b"icon32badmask\0Xfwl4WmHintsIconTest\0",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Depth24 => "WM_HINTS icon: 24bpp pixmap + 1bpp mask",
            Self::Depth32 => "WM_HINTS icon: 32bpp pixmap + 1bpp mask",
            Self::Depth32NoMask => "WM_HINTS icon: 32bpp pixmap, alpha only, no mask",
            Self::Depth32BadMask => "WM_HINTS icon: 32bpp pixmap + undersized mask",
        }
    }

    /// Left panel is what the compositor should show, right panel is the contrast:
    /// usually the likeliest way to get it wrong, but for the mismatched mask both
    /// panels are acceptable outcomes.
    fn panel_labels(self) -> (&'static [u8], &'static [u8]) {
        match self {
            Self::Depth24 | Self::Depth32 => (b"as sent (correct)", b"mask ignored (wrong)"),
            Self::Depth32NoMask => (b"as sent (correct)", b"alpha ignored (wrong)"),
            Self::Depth32BadMask => (b"mask dropped (ok)", b"icon dropped (also ok)"),
        }
    }
}

fn glyph(digit: u8) -> [&'static str; GLYPH_H as usize] {
    match digit {
        b'2' => ["#####", "....#", "....#", "#####", "#....", "#....", "#####"],
        b'3' => ["#####", "....#", "....#", "#####", "....#", "....#", "#####"],
        b'4' => ["#...#", "#...#", "#...#", "#####", "....#", "....#", "....#"],
        b'A' => ["#####", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
        b'X' => ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
        _ => ["#####", "#...#", "#...#", "#...#", "#...#", "#...#", "#####"],
    }
}

fn glyph_lit(digits: &[u8; 2], x: u16, y: u16) -> bool {
    match (x.checked_sub(GLYPH_LEFT), y.checked_sub(*DIGIT_ROWS.start())) {
        (Some(dx), Some(dy)) if dx < GLYPH_ADVANCE * 2 && dy < GLYPH_H * GLYPH_SCALE && dx % GLYPH_ADVANCE < GLYPH_W * GLYPH_SCALE => {
            let rows = glyph(digits[usize::from(dx / GLYPH_ADVANCE)]);
            rows[usize::from(dy / GLYPH_SCALE)].as_bytes()[usize::from(dx % GLYPH_ADVANCE / GLYPH_SCALE)] == b'#'
        }
        _ => false,
    }
}

/// True where `icon_mask` has a 1 bit, i.e. where the icon is drawn.
fn icon_opaque(x: u16, y: u16) -> bool {
    let top_left = x + y < BEVEL_TOP_LEFT;
    let top_right = (ICON_LAST - x) + y < BEVEL_TOP_RIGHT;
    let bottom_right = (ICON_LAST - x) + (ICON_LAST - y) < BEVEL_BOTTOM_RIGHT;
    let notch = NOTCH_ROWS.contains(&y) && x >= NOTCH_LEFT;
    let comb = COMB_ROWS.contains(&y) && x % COMB_PERIOD < COMB_SLOT;
    !(top_left || top_right || bottom_right || notch || comb)
}

/// Straight (non-premultiplied) RGBA of the icon at `(x, y)`.
fn icon_texel(kind: IconKind, x: u16, y: u16) -> [u8; 4] {
    if !icon_opaque(x, y) {
        // With no mask, the cut-out has to come from the alpha channel instead, and
        // premultiplication leaves nothing of the magenta behind to give it away.
        if kind.has_mask() { MAGENTA } else { TRANSPARENT }
    } else if x == 0 || x == ICON_LAST {
        WHITE
    } else if y == 0 {
        YELLOW
    } else if y == ICON_LAST {
        CYAN
    } else if SWATCH_ROWS.contains(&y) {
        if x < ICON_SIZE / 3 {
            RED
        } else if x < ICON_SIZE * 2 / 3 {
            GREEN
        } else {
            BLUE
        }
    } else if DIGIT_ROWS.contains(&y) {
        if glyph_lit(kind.digits(), x, y) { WHITE } else { SLATE }
    } else if RAMP_ROWS.contains(&y) {
        let ramp = (u32::from(x) * 0xff / u32::from(ICON_LAST)) as u8;
        match kind {
            IconKind::Depth24 => [ramp, ramp, ramp, 0xff],
            IconKind::Depth32 | IconKind::Depth32NoMask | IconKind::Depth32BadMask => [0xff, 0xff, 0xff, ramp],
        }
    } else if COMB_ROWS.contains(&y) {
        ORANGE
    } else {
        BLACK
    }
}

fn premultiply(value: u8, alpha: u8) -> u8 {
    ((u16::from(value) * u16::from(alpha) + 127) / 255) as u8
}

fn over(src: u8, alpha: u8, dst: u8) -> u8 {
    ((u16::from(src) * u16::from(alpha) + u16::from(dst) * u16::from(0xff - alpha) + 127) / 255) as u8
}

fn each_pixel(width: u16, height: u16) -> impl Iterator<Item = (u16, u16)> {
    (0..height).flat_map(move |y| (0..width).map(move |x| (x, y)))
}

fn color_image(kind: IconKind, setup: &Setup) -> Image<'static> {
    let blank = Image::allocate_native(ICON_SIZE, ICON_SIZE, kind.depth(), setup).unwrap();
    each_pixel(ICON_SIZE, ICON_SIZE).fold(blank, |mut image, (x, y)| {
        let [r, g, b, a] = icon_texel(kind, x, y);
        let pixel = match kind {
            // Depth 24 has no alpha channel, and the padding byte is deliberately left
            // zero: a compositor that mistakes the pad for alpha renders nothing at all.
            IconKind::Depth24 => u32::from_be_bytes([0x00, r, g, b]),
            IconKind::Depth32 | IconKind::Depth32NoMask | IconKind::Depth32BadMask => {
                u32::from_be_bytes([a, premultiply(r, a), premultiply(g, a), premultiply(b, a)])
            }
        };
        image.put_pixel(x, y, pixel);
        image
    })
}

fn mask_image(setup: &Setup, size: u16) -> Image<'static> {
    let blank = Image::allocate_native(size, size, 1, setup).unwrap();
    let scale = ICON_SIZE / size;
    each_pixel(size, size).fold(blank, |mut image, (x, y)| {
        image.put_pixel(x, y, u32::from(icon_opaque(x * scale, y * scale)));
        image
    })
}

/// What the compositor should show, next to what it looks like when the transparency
/// is dropped: the mask for the masked icons, the alpha channel for the unmasked one.
fn window_pixel(kind: IconKind, x: u16, y: u16) -> u32 {
    let checker = if (x / CHECKER_SIZE + y / CHECKER_SIZE).is_multiple_of(2) {
        CHECKER_DARK
    } else {
        CHECKER_LIGHT
    };
    let panel = PANEL_LEFT
        .iter()
        .position(|left| (*left..left + PANEL_SIZE).contains(&x) && (PANEL_TOP..PANEL_TOP + PANEL_SIZE).contains(&y));
    match panel {
        Some(index) => {
            let sx = (x - PANEL_LEFT[index]) / REFERENCE_SCALE;
            let sy = (y - PANEL_TOP) / REFERENCE_SCALE;
            let [r, g, b, a] = icon_texel(kind, sx, sy);
            let (r, g, b, alpha) = match (index, kind) {
                // The undersized mask cannot be honored at all, so both outcomes we are
                // willing to accept get a panel: the bare pixmap, or no icon.
                (0, IconKind::Depth32BadMask) => (r, g, b, a),
                (1, IconKind::Depth32BadMask) => (r, g, b, 0x00),
                (0, _) if !icon_opaque(sx, sy) => (r, g, b, 0x00),
                // The pixmap holds premultiplied color, so discarding alpha shows what
                // was premultiplied rather than the straight color: the ramp collapses
                // to a luminance ramp and the cut-outs go black.
                (1, _) if !kind.has_mask() => (premultiply(r, a), premultiply(g, a), premultiply(b, a), 0xff),
                _ => (r, g, b, a),
            };
            u32::from_be_bytes([0x00, over(r, alpha, checker), over(g, alpha, checker), over(b, alpha, checker)])
        }
        None => u32::from_be_bytes([0x00, checker, checker, checker]),
    }
}

fn window_image(kind: IconKind, depth: u8, setup: &Setup) -> Image<'static> {
    let blank = Image::allocate_native(WIN_W, WIN_H, depth, setup).unwrap();
    each_pixel(WIN_W, WIN_H).fold(blank, |mut image, (x, y)| {
        image.put_pixel(x, y, window_pixel(kind, x, y));
        image
    })
}

fn upload_pixmap<C: Connection>(conn: &C, root: u32, image: &Image<'_>) -> u32 {
    let pixmap = conn.generate_id().unwrap();
    conn.create_pixmap(image.depth(), pixmap, root, image.width(), image.height())
        .unwrap();
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, pixmap, &CreateGCAux::new()).unwrap();
    image.put(conn, pixmap, gc, 0, 0).unwrap();
    conn.free_gc(gc).unwrap();
    pixmap
}

struct Atoms {
    wm_protocols: u32,
    wm_delete_window: u32,
}

struct IconWindow {
    kind: IconKind,
    window: u32,
    gc: u32,
    text_gc: Option<u32>,
    content: Image<'static>,
}

fn create_icon_window<C: Connection>(conn: &C, screen: &Screen, kind: IconKind, x: i16, font: Option<u32>, atoms: &Atoms) -> IconWindow {
    let setup = conn.setup();
    let window = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        x,
        100,
        WIN_W,
        WIN_H,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY),
    )
    .unwrap();

    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        kind.title().as_bytes(),
    )
    .unwrap();
    conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, kind.wm_class())
        .unwrap();
    conn.change_property32(
        PropMode::REPLACE,
        window,
        atoms.wm_protocols,
        AtomEnum::ATOM,
        &[atoms.wm_delete_window],
    )
    .unwrap();

    let icon_pixmap = upload_pixmap(conn, screen.root, &color_image(kind, setup));
    let icon_mask = kind
        .has_mask()
        .then(|| upload_pixmap(conn, screen.root, &mask_image(setup, kind.mask_size())));

    WmHints {
        input: Some(true),
        initial_state: Some(WmHintsState::Normal),
        icon_pixmap: Some(icon_pixmap),
        icon_mask,
        ..WmHints::new()
    }
    .set(conn, window)
    .unwrap();

    let mask_note = match icon_mask {
        Some(icon_mask) => format!("icon_mask 0x{icon_mask:x} (depth 1, {}x{})", kind.mask_size(), kind.mask_size()),
        None => "no icon_mask".to_string(),
    };
    eprintln!(
        "  {}: window 0x{window:x}  icon_pixmap 0x{icon_pixmap:x} (depth {})  {mask_note}",
        String::from_utf8_lossy(kind.digits()),
        kind.depth(),
    );

    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, window, &CreateGCAux::new()).unwrap();

    let text_gc = font.map(|font| {
        let text_gc = conn.generate_id().unwrap();
        conn.create_gc(
            text_gc,
            window,
            &CreateGCAux::new().font(font).foreground(0x00ffffff).background(0x00202020),
        )
        .unwrap();
        text_gc
    });

    IconWindow {
        kind,
        window,
        gc,
        text_gc,
        content: window_image(kind, screen.root_depth, setup),
    }
}

fn draw<C: Connection>(conn: &C, icon_window: &IconWindow) {
    icon_window.content.put(conn, icon_window.window, icon_window.gc, 0, 0).unwrap();
    if let Some(text_gc) = icon_window.text_gc {
        let (left, right) = icon_window.kind.panel_labels();
        conn.image_text8(icon_window.window, text_gc, PANEL_LEFT[0] as i16, LABEL_BASELINE, left)
            .unwrap();
        conn.image_text8(icon_window.window, text_gc, PANEL_LEFT[1] as i16, LABEL_BASELINE, right)
            .unwrap();
    }
}

fn print_legend() {
    eprintln!(
        "\
Four windows set WM_HINTS.icon_pixmap and nothing else icon related, so there is no
_NET_WM_ICON to fall back to. \"24bpp\" and \"32bpp\" are X depths: depth 24 is stored 32
bits per pixel with an unused padding byte, depth 32 is ARGB with the alpha channel
premultiplied into the color channels.

  \"24\"  depth 24 + 48x48 icon_mask  transparency comes only from the mask
  \"32\"  depth 32 + 48x48 icon_mask  mask and alpha channel agree
  \"3A\"  depth 32, no icon_mask      transparency comes only from the alpha channel
  \"3X\"  depth 32 + 24x24 icon_mask  malformed on purpose; see below

The first three are the same picture and must render identically. Any difference
between them is a bug in one of the three paths.

\"3X\" is the abuse case. Nothing in X requires icon_mask to match icon_pixmap, so a
mask that is smaller than the pixmap is a thing any client can hand over, and a
compositor that indexes the mask with the pixmap's coordinates will run off the end
of it. Surviving that is the test; either of these is a fine result:
  - the mask is dropped and the bare pixmap is shown (magenta and all)
  - the icon is dropped and a themed fallback icon is used instead
Anything else, including the compositor dying, is a bug.

Each window renders its icon at 4x over a checkerboard: the left panel is what the
titlebar/task list/alt-tab should look like, the right panel is what it looks like
when the transparency is dropped (the mask for \"24\"/\"32\", the alpha for \"3A\").

Icon design, 48x48:
  frame       1px, yellow along the top, cyan along the bottom, white on the sides
  rows  1-10  swatches: red | green | blue, left to right
  rows 12-32  the label, white on dark slate
  rows 34-41  24bpp: black->white gray ramp; 32bpp: white with alpha 0->255
  rows 42-46  solid orange
  magenta     painted everywhere, and only where, icon_mask says transparent
              (\"3A\" has no mask, so its cut-outs are alpha 0 and show no magenta;
               \"3X\" cannot have its mask honored, so magenta there is expected)

Cut-out shape, identical in the mask and in \"3A\"'s alpha channel:
  corner bevels shrinking clockwise: top-left 12px, top-right 8px, bottom-right 4px,
    bottom-left square
  an 8x8 bite out of the right edge, vertically centered
  2 transparent columns at the start of every 8 in the bottom orange band

How each mistake shows up:
  magenta anywhere                icon_mask was not applied
  only magenta is visible         icon_mask was inverted (0 treated as opaque)
  magenta smears diagonally       mask scanline padding ignored: 48 bits per row is
                                    6 bytes of data padded out to 8
  slots at the end of each        mask bit order swapped (MSB-first vs LSB-first)
    8px group instead of the
    start
  red and blue swatches swapped   pixel byte/channel order wrong
  yellow edge along the bottom    icon flipped vertically
  digits read \"42\" / \"23\"         icon flipped horizontally
  bevels on the wrong corners     icon rotated
  24bpp icon fully invisible      the depth-24 padding byte was used as alpha
  32bpp ramp is a flat white bar  alpha channel ignored
  32bpp ramp too dark or ringed   premultiplied vs straight alpha mismatch
  either icon has ragged edges    pixmap and mask disagree on size or stride
  \"3A\" square with black bevels   alpha ignored when there is no mask to consult
  \"3A\" missing entirely           a mask was required rather than treated as optional
  compositor dies on \"3X\"         the mask is indexed with the pixmap's coordinates
                                    without checking that the two sizes agree
  \"3X\" beveled like the others    a 24x24 mask was scaled up to fit rather than
                                    rejected, i.e. data was invented

Press Escape or close all four windows to exit."
    );
}

fn main() {
    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];

    if !setup.pixmap_formats.iter().any(|format| format.depth == 32) {
        eprintln!("This X server advertises no depth-32 pixmap format; the 32bpp half of this test cannot run.");
        std::process::exit(1);
    }

    let root_visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == screen.root_visual);
    match root_visual {
        Some(visual) if (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0x00ff00, 0x0000ff) => {
            eprintln!(
                "warning: root visual masks are r={:#010x} g={:#010x} b={:#010x}, not plain RGB; \
                 the colors described below will not match what is drawn",
                visual.red_mask, visual.green_mask, visual.blue_mask,
            );
        }
        _ => {}
    }

    let atoms = Atoms {
        wm_protocols: conn.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom,
        wm_delete_window: conn.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom,
    };

    let font = conn.generate_id().unwrap();
    let font = match conn.open_font(font, b"fixed").unwrap().check() {
        Ok(()) => Some(font),
        Err(err) => {
            eprintln!("note: could not open the \"fixed\" font ({err}); the in-window panel labels will be omitted");
            None
        }
    };

    print_legend();
    eprintln!();

    let mut windows: Vec<IconWindow> = [
        IconKind::Depth24,
        IconKind::Depth32,
        IconKind::Depth32NoMask,
        IconKind::Depth32BadMask,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, kind)| {
        let x = 80 + index as i16 * (WIN_W as i16 + 40);
        create_icon_window(&conn, screen, kind, x, font, &atoms)
    })
    .collect();

    let mapping = conn
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
        .unwrap()
        .reply()
        .unwrap();
    let keysyms_per_keycode = mapping.keysyms_per_keycode as usize;
    let escape_keycode = mapping
        .keysyms
        .chunks(keysyms_per_keycode)
        .position(|keysyms| keysyms.first() == Some(&XK_ESCAPE))
        .map(|index| setup.min_keycode + index as u8);

    windows.iter().for_each(|icon_window| {
        conn.map_window(icon_window.window).unwrap();
    });
    conn.flush().unwrap();

    while !windows.is_empty() {
        let event = conn.wait_for_event().unwrap();
        match event {
            Event::Expose(e) if e.count == 0 => {
                if let Some(icon_window) = windows.iter().find(|w| w.window == e.window) {
                    draw(&conn, icon_window);
                    conn.flush().unwrap();
                }
            }
            Event::KeyPress(e) if Some(e.detail) == escape_keycode => {
                eprintln!("Escape pressed, exiting");
                break;
            }
            Event::ClientMessage(e) if e.type_ == atoms.wm_protocols && e.data.as_data32()[0] == atoms.wm_delete_window => {
                eprintln!("WM_DELETE_WINDOW for 0x{:x}", e.window);
                windows.retain(|icon_window| icon_window.window != e.window);
                conn.destroy_window(e.window).unwrap();
                conn.flush().unwrap();
            }
            _ => {}
        }
    }
}

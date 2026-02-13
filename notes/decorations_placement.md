# Window Decoration Placement Algorithm

This document describes how to position window decoration textures around a window in xfwl4, based on the xfwm4 implementation in `src/frame.c`.

## Coordinate System

All coordinates are relative to the **decoration frame**, not the window content:
- `(0, 0)` is the top-left corner of the frame (including decorations)
- Window content starts at `(frame_left, frame_top)` within this coordinate system

## Window States

Decorations visibility depends on window state:

1. **Fullscreen windows**: No decorations at all
2. **Maximized windows** (with `borderless_maximize` enabled):
   - Show titlebar and buttons
   - Hide all borders and corners
   - Use `maximized_offset` instead of `button_offset` for button positioning
3. **Normal/tiled windows**: Show all decorations

## Decoration Dimensions

From theme textures (active state preferred for measurements):

- `frame_left`: Width of `left-active.xpm` (or `left-active.png` if stretch variant exists)
- `frame_right`: Width of `right-active.xpm` (or stretch variant)
- `frame_top`: Height from titlebar textures (see "Titlebar Background" section for precedence)
- `frame_bottom`: Height of `bottom-active.xpm` (or stretch variant)

Corner dimensions are taken from the corner textures:
- `corner_top_left_{width,height}`: From `top-left-active.xpm`
- `corner_top_right_{width,height}`: From `top-right-active.xpm`
- `corner_bottom_left_{width,height}`: From `bottom-left-active.xpm`
- `corner_bottom_right_{width,height}`: From `bottom-right-active.xpm`

If stretch variants exist (e.g., `top-left-active.png`), use those dimensions instead.

## Configuration Settings

From `Xfwl4Config` (xfconf channel `xfwm4`):

- `button_layout`: String defining button order and position (e.g., "O|HMC")
  - Characters before `|` appear on the left
  - Characters after `|` appear on the right
  - Button letters: O=menu, H=minimize, M=maximize, C=close, S=shade, T=stick
- `button_spacing`: Pixels between buttons (default: 0)
- `button_offset`: Pixels from frame edge to first button (default: 0)
- `maximized_offset`: Like `button_offset` but for maximized windows (default: 0)
- `title_alignment`: Left, Center, or Right alignment for window title text
- `title_horizontal_offset`: Horizontal offset for title text (default: 0)
- `title_vertical_offset_active`: Vertical offset for title text when window is active (default: 0)
- `title_vertical_offset_inactive`: Vertical offset for title text when window is inactive (default: 0)
- `full_width_title`: Whether title spans full width between buttons (default: true)
- `frame_border_top`: Top border height for maximized windows (default: 0)

All of these properties are already implemented in `Xfwl4Config` and can be read from both xfconf and theme `themerc` files.

## Placement Algorithm

### 1. Corners (Fixed Size, Never Tile)

**Top-Left Corner:**
- Position: `(0, 0)`
- Size: `(corner_top_left_width, corner_top_left_height)`
- Texture: `top-left-active.xpm` or `top-left-inactive.xpm`

**Top-Right Corner:**
- Position: `(total_frame_width - corner_top_right_width, 0)`
- Size: `(corner_top_right_width, corner_top_right_height)`
- Texture: `top-right-active.xpm` or `top-right-inactive.xpm`

**Bottom-Left Corner:**
- Position: `(0, total_frame_height - corner_bottom_left_height)`
- Size: `(corner_bottom_left_width, corner_bottom_left_height)`
- Texture: `bottom-left-active.xpm` or `bottom-left-inactive.xpm`

**Bottom-Right Corner:**
- Position: `(total_frame_width - corner_bottom_right_width, total_frame_height - corner_bottom_right_height)`
- Size: `(corner_bottom_right_width, corner_bottom_right_height)`
- Texture: `bottom-right-active.xpm` or `bottom-right-inactive.xpm`

### 2. Borders (Tile Vertically or Horizontally)

**Left Border:**
- Position: `(0, frame_top)`
- Size: `(frame_left, window_height + frame_bottom - corner_bottom_left_height)`
- Texture: `left-active.xpm` or `left-inactive.xpm`
- **Tiling**: Vertical only (repeat texture vertically to fill height)
- Note: `window_height` is the client window content height (not including decorations)

**Right Border:**
- Position: `(total_frame_width - frame_right, frame_top)`
- Size: `(frame_right, window_height + frame_bottom - corner_bottom_right_height)`
- Texture: `right-active.xpm` or `right-inactive.xpm`
- **Tiling**: Vertical only
- Note: `window_height` is the client window content height (not including decorations)

**Bottom Border:**
- Position: `(corner_bottom_left_width, total_frame_height - frame_bottom)`
- Size: `(total_frame_width - corner_bottom_left_width - corner_bottom_right_width, frame_bottom)`
- Texture: `bottom-active.xpm` or `bottom-inactive.xpm`
- **Tiling**: Horizontal only (repeat texture horizontally to fill width)

### 3. Titlebar Background

**Important naming clarification:**
- There is **NO** `top-active.xpm` (without a number) - "top" as a side decoration is unused
- There **ARE** `top-{1-5}-active.xpm` (with numbers) - these are optional overlays for title parts
- The single stretched variant is `title-active-stretch`, **NOT** `top-active-stretch`

Themes provide titlebar graphics in **two modes**, with an optional overlay system:

#### Mode 1: Single Stretched Title Texture

**Files to check for:**
- `title-active-stretch.xpm` or `title-active-stretch.png`
- `title-inactive-stretch.xpm` or `title-inactive-stretch.png`

If these files exist:
- Use as a **single stretched/tiled texture** for the entire titlebar
- Position: `(corner_top_left_width, 0)`
- Size: `(total_frame_width - corner_top_left_width - corner_top_right_width, frame_top)`
- **Tiling**: Horizontal only
- Title text is rendered directly on top with a clipping rectangle
- **No 5-part division needed**
- `frame_top` = height of `title-active-stretch` texture

**Important:** The filename is `title-active-stretch`, NOT `top-active-stretch`. There is no "top" side decoration (it's explicitly unused in xfwm4).

**When this mode is used:** If `title-active-stretch.xpm` exists, the 5-part system (Mode 2) is completely bypassed. The `top-{1-5}` overlay files are ignored even if they exist.

#### Mode 2: Traditional 5-Part Title System

**When this mode is used:** Only if `title-active-stretch.xpm` does NOT exist. If the stretch variant exists, this entire mode is skipped.

**Files to check for (in order of priority):**

For each part (1-5), xfwm4 loads both:
1. `title-{1-5}-active.xpm` and `title-{1-5}-inactive.xpm` (base textures)
2. `top-{1-5}-active.xpm` and `top-{1-5}-inactive.xpm` (optional overlays)

When rendering, for each part:
- If `top-{part}-active.xpm` exists → use it
- Otherwise → use `title-{part}-active.xpm` as fallback

**Important notes:**
- `top-{1-5}` are **numbered overlay files** that can override individual title parts, NOT a single "top-active.xpm" file (which doesn't exist)
- `top-{1-5}` files are **ONLY used in Mode 2** (5-part). They are completely ignored if `title-active-stretch` exists.
- A `top-{part}` overlay is only useful if the corresponding `title-{part}` base texture exists, since the base texture is always accessed during rendering.

**Calculating Part Widths:**

First, determine button areas:
- `left_button_area`: X position where left buttons end + `button_spacing`
- `right_button_area`: X position where right buttons start
- `available_width`: `total_frame_width - corner_top_left_width - corner_top_right_width`

For `full_width_title` mode (title spans entire available width):
- `w1 = left_button_area - corner_top_left_width`
- `w2 = title-2 texture width`
- `w4 = title-4 texture width`
- `w5 = available_width - right_button_area`
- `w3 = available_width - w1 - w2 - w4 - w5` (middle section for title text)

For non-`full_width_title` mode (title only as wide as text):
- `w3 = title_text_pixel_width + title_shadow_offset`
- Calculate `w1`, `w2`, `w4` based on title alignment (left/center/right)
- `w5 = remaining width`

**Title Part Positions:**
- TITLE_1: `(corner_top_left_width, 0)`, width `w1` - left edge filler (may be omitted if `w1 = 0`)
- TITLE_2: After TITLE_1, width `w2` - left title decoration
- TITLE_3: After TITLE_2, width `w3` - **middle section with title text** (TILE HORIZONTALLY)
- TITLE_4: After TITLE_3, width `w4` - right title decoration
- TITLE_5: After TITLE_4, width `w5` - right edge filler (may be omitted if `w5 = 0`)

All titlebar parts:
- Height: `frame_top`
- **Tiling**: Horizontal only (to fill calculated widths)
- Textures: Use `top-{part}` if exists, otherwise `title-{part}`

**Required textures (for theme authoring):**
- All parts `title-{1,2,3,4,5}-active.xpm` and `-inactive.xpm` should be provided
- `title-3` is used to measure `frame_top` height
- At runtime, parts 1, 3, and 5 may not be rendered if the window is too narrow (w1/w3/w5 = 0), but the theme cannot assume windows will always be narrow

**Optional overlay example:**
A theme could provide:
- `title-{1-5}-active.xpm` (base for all parts)
- `top-2-active.xpm` and `top-4-active.xpm` (fancy decorative overlays for left/right)
- Parts 1, 3, 5 use their `title-{part}` base textures
- Parts 2, 4 use the fancier `top-{part}` overlays

#### Determining `frame_top` (Titlebar Height)

Priority for measuring titlebar height:
1. If `title-active-stretch.xpm` exists → use its height
2. Otherwise, use `title-3-active.xpm` height (this file must exist)

### 4. Buttons

Buttons are positioned based on `button_layout` configuration.

**Button Dimensions:**
- Width and height from button texture files (e.g., `close-active.xpm`)
- All buttons for a given type (close, maximize, etc.) should have the same size across states

**Left-Side Buttons:**

Starting position: `frame_left + button_offset`

For each button character before `|` in `button_layout`:
1. If button fits (current_x + button_width + button_spacing < right_button_area):
   - Position button at `(current_x, vertical_center)`
   - Advance: `current_x += button_width + button_spacing`
2. Otherwise: Hide button (doesn't fit)

**Right-Side Buttons:**

Starting position: `total_frame_width - frame_right - button_offset`

For each button character after `|` in `button_layout` (iterate right-to-left):
1. Calculate position: `x = current_x - button_width - button_spacing`
2. If button fits (x > left_button_area):
   - Position button at `(x, vertical_center)`
   - Advance: `current_x = x`
3. Otherwise: Hide button (doesn't fit)

**Vertical Centering:**

Button Y position: `(frame_top - button_height + 1) / 2`

This centers the button vertically within the titlebar.

**Button Textures:**
- Active window: `{button}-active.xpm`
- Inactive window: `{button}-inactive.xpm`
- Hover state: `{button}-prelight.xpm`
- Pressed state: `{button}-pressed.xpm`

Buttons are **never tiled** - they are fixed-size textures that should be stretched if necessary.

### 5. Title Text Rendering

Title text is rendered on top of TITLE_3 (middle titlebar section):

**Horizontal Position:**
- Depends on `title_alignment` (Left/Center/Right)
- Add `title_horizontal_offset` to the calculated position
- Clip to TITLE_3 region to prevent text overflow into buttons

**Vertical Position:**
- Calculate text height using Pango layout
- `title_y = title_vertical_offset + (frame_top - text_height) / 2`
- If text would overflow: `title_y = MAX(0, frame_top - text_height)`

**Text Shadow:**
- If `title_shadow_active` or `title_shadow_inactive` is enabled
- Draw shadow first (offset by 1px), then draw text on top
- Shadow type: UNDER (below+right) or FRAME (all 4 sides)

## Special Cases

### Shaded Windows
- Hide left and right borders
- Keep titlebar, top corners, bottom border, and bottom corners

### Maximized Windows (borderless_maximize enabled)
- Hide all borders and corners
- Keep only titlebar and buttons
- Use `maximized_offset` instead of `button_offset`
- May hide a border strip at the very top (frame_border_top) for visual consistency

## Texture Loading Strategy

For the Rust implementation:

1. **Load once at theme load time:**
   - All corner textures (check for `-stretch` variants, can be `.png` or `.xpm`)
   - All border textures (check for `-stretch` variants)
   - Titlebar textures (see below)
   - All button textures (all states: active, inactive, prelight, pressed)

2. **Titlebar texture loading logic:**

   **Note:** xfwm4 actually loads all files it can find (both stretch and 5-part), but at render time it uses XOR logic - if stretch exists, 5-part is ignored. You can optimize by only loading what you'll use:

   ```rust
   // First, try single stretched variant
   if title-active-stretch.{xpm,png} exists:
       Load title-{active,inactive}-stretch
       → Use Mode 1 (single texture)
       → Skip loading title-{1-5} and top-{1-5} (won't be used)
   else:
       // Load 5-part system
       For each part (1-5):
           Load title-{part}-{active,inactive}.xpm
           // Only load top-{part} if corresponding title-{part} exists
           If title-{part} loaded successfully:
               Optionally load top-{part}-{active,inactive}.xpm (overlay)

       Ensure title-3-active.xpm exists (required fallback)
       → Use Mode 2 (5-part with optional overlays)
   ```

   **Required textures by mode (for theme authoring):**
   - Mode 1: `title-active-stretch.xpm` and `title-inactive-stretch.xpm`
   - Mode 2: All 5 parts `title-{1,2,3,4,5}-active.xpm` and `-inactive.xpm`
     - `title-3` is also used to measure `frame_top` height
     - At runtime: parts 1, 3, and 5 may be skipped if window is too narrow (w1/w3/w5 = 0)
     - At runtime: parts 2 and 4 are always rendered
     - Any `top-{1-5}` overlays are optional (only if corresponding `title-{part}` exists)

3. **Rendering behavior (XOR):**
   At render time, only ONE mode is used:
   - If `title-active-stretch` texture exists → Use Mode 1, ignore all `title-{1-5}` and `top-{1-5}` textures
   - Otherwise → Use Mode 2 with the `title-{1-5}` and optional `top-{1-5}` overlays

   Even if a theme provides both stretch and 5-part textures, only the stretch variant will be rendered.

4. **File naming reference:**
   - ✓ `title-active-stretch.xpm` - Single stretched titlebar
   - ✓ `title-{1-5}-active.xpm` - 5-part base textures
   - ✓ `top-{1-5}-active.xpm` - Optional overlays for parts 1-5 (only used if stretch doesn't exist)
   - ✗ `top-active.xpm` - Does NOT exist (would be a "side", which is unused)
   - ✗ `top-active-stretch.xpm` - Does NOT exist (use `title-active-stretch`)

5. **Upload to GPU once:**
   - Create `GlesTexture` for each loaded texture
   - These are cached in `DecorationTheme`

6. **At render time:**
   - Create `TextureRenderElement` instances with appropriate positions/sizes
   - Wrap tiled elements in `TextureShaderElement` with tiling shader
   - Use non-tiled elements directly for corners and buttons
   - Damage tracking (via cached `Id` values) prevents unnecessary GPU work

## Render Element Stacking Order

Reference: `~/src/xfce/xfwm4/src/client.c`, `clientFrame()` (~line 1961)

In xfwm4, decoration X child windows are created in a specific order inside the frame
window. X stacks later-created siblings on top of earlier ones, so creation order
determines the visual stacking.

**xfwm4 creation order (bottom → top):**

1. `sides[SIDE_LEFT]`
2. `sides[SIDE_RIGHT]`
3. `sides[SIDE_BOTTOM]`
4. `corners[CORNER_BOTTOM_LEFT]`
5. `corners[CORNER_BOTTOM_RIGHT]`
6. `corners[CORNER_TOP_LEFT]`
7. `corners[CORNER_TOP_RIGHT]`
8. `title` (titlebar background + text)
9. `sides[SIDE_TOP]` — created **after** title because the two overlap and top must win
10. `buttons[0..5]` (MENU, STICK, SHADE, HIDE, MAXIMIZE, CLOSE)
11. `c->window` (client surface) — explicitly `XRaiseWindow`'d above all decorations

**Corresponding order for `ssd.rs` render elements (first = drawn first = bottom):**

1. Left border
2. Right border
3. Bottom border
4. Bottom-left corner
5. Bottom-right corner
6. Top-left corner
7. Top-right corner
8. Titlebar background (title parts 1–5 or stretch, plus title text)
9. Top border
10. Buttons
11. Client window surface (returned by the window element itself, above decorations)

The key non-obvious constraint is that the top border must come **after** the titlebar
in the element list, because they can overlap and the top border should paint over the
titlebar edge.

## Implementation Notes

- `window_width` and `window_height` refer to the client window's content dimensions (excluding all decorations)
- Total frame dimensions: `window_width + frame_left + frame_right` × `window_height + frame_top + frame_bottom`
- Window content area is inset by `(frame_left, frame_top)` from frame origin
- All measurements use logical pixels; apply scale factor when converting to physical pixels
- Theme textures may be very small (4×29 pixels for titlebars) and are tiled to fill larger areas
- The shader-based tiling approach uploads small textures once and tiles on GPU, much more efficient than CPU-side tiling

## Scaling and Fractional Scale Issues

### How xfwm4 Handles Scale

xfwm4 does NOT scale decoration textures. Every texture pixel maps 1:1 to a
screen pixel (`buffer_scale = 1`). At HiDPI, xfwm4 relies on theme selection:
`settings.c:getThemeName()` automatically substitutes `Default-xhdpi` when the
GDK scale factor is > 1 and the selected theme is `Default`. For all other
themes, the user is expected to select an appropriately-sized theme.

The only place xfwm4 uses the scale factor in decoration rendering is for pango
title text (`screen.c:myScreenUpdateFontAttr()`), where
`pango_attr_scale_new(scale)` is applied so the title text renders at the
correct resolution.

### Our Approach

We follow xfwm4's model: `buffer_scale = 1` for all decoration textures. Texture
buffer dimensions are used directly as logical dimensions. The compositor's
output scale only affects the physical rendering resolution (logical coordinates
are multiplied by the output scale to produce physical pixel positions and
sizes).

The `scale` field on `WindowDecorations` is retained solely for pango text
rendering.

### Fractional Scale Rounding Problem

At fractional output scales (e.g., 2.25), decoration elements can be misaligned
by up to 1 physical pixel. This is a fundamental limitation of fractional
scaling with independently-positioned elements.

**Root cause:** smithay computes each element's physical rectangle independently.
When an element's position or size involves a fractional physical value, it gets
rounded to the nearest integer pixel. Two adjacent elements that share a logical
edge can round to different physical pixel boundaries:

```
round(a * scale) + round(b * scale) ≠ round((a + b) * scale)
```

**Concrete example** (Default theme, scale 2.25, window_w = 100):
- Left side: width 5 logical → physical 0 to round(11.25) = 11 → **11px wide**
- Right side: position round(105 * 2.25) = 236, width round(5 * 2.25) = 11
  → right edge at 247
- But frame right edge: round(110 * 2.25) = round(247.5) = 248
- **1px gap** at the right outer edge

The same issue can cause inner-edge misalignment (decoration piece extends 1px
too far into the window content area) depending on the window size. Whether the
error manifests on the inner or outer edge depends on the specific combination
of window dimensions and scale factor.

**Currently accepted:** This sub-pixel imperfection is left as-is. Most Wayland
compositors have similar artifacts at fractional scales. The window content
typically masks inner-edge errors, and outer-edge errors are at most 1 physical
pixel.

### Future Fix: Option 1 — Physical Pixel Grid Pre-computation

Compute all key physical pixel boundaries first, then derive each element's
position and size from them, ensuring adjacent elements share exact edges.

**Algorithm:**

1. Compute the physical coordinates of all decoration boundaries:
   ```
   left_outer     = round(0)
   left_inner     = round(frame_left * scale)
   right_inner    = round((frame_left + window_w) * scale)
   right_outer    = round((frame_left + window_w + frame_right) * scale)
   top_outer      = round(0)
   top_inner      = round(frame_top * scale)
   bottom_inner   = round((frame_top + window_h) * scale)
   bottom_outer   = round((frame_top + window_h + frame_bottom) * scale)
   corner_tl_right = round(corner_top_left_w * scale)
   corner_tr_left  = right_outer - round(corner_top_right_w * scale)
   corner_bl_right = round(corner_bottom_left_w * scale)
   corner_bl_top   = bottom_outer - round(corner_bottom_left_h * scale)
   corner_br_left  = right_outer - round(corner_bottom_right_w * scale)
   corner_br_top   = bottom_outer - round(corner_bottom_right_h * scale)
   ```

2. Derive each element's physical position and size from these boundaries:
   ```
   left_side:   x = left_outer,     w = left_inner - left_outer
                y = top_inner,      h = corner_bl_top - top_inner
   right_side:  x = right_inner,    w = right_outer - right_inner
                y = top_inner,      h = corner_br_top - top_inner
   bottom:      x = corner_bl_right, w = corner_br_left - corner_bl_right
                y = bottom_inner,    h = bottom_outer - bottom_inner
   ```

3. Convert physical sizes back to logical for smithay's API:
   ```
   logical_size = physical_size / scale  (as f64, passed to smithay)
   ```

**Challenges:**
- smithay's `TextureRenderElement::from_static_texture` takes
  `size: Option<Size<i32, Logical>>` — integer logical size. Converting a
  physical size back to logical and rounding to integer may re-introduce the
  original problem. This may require using `Size<f64, Logical>` if smithay
  supports it, or patching smithay.
- The tiling shader's `geo_size` uniform would need to use the physical pixel
  size directly rather than computing it from logical * scale.
- The `src` rectangle computation in `create_texture_elem` would need adjustment
  to match the new sizing approach.

### Future Fix: Option 2 — Single-Buffer Decoration Rendering

Render all decoration pieces into a single offscreen buffer, then present that
buffer as one element. This eliminates inter-element rounding entirely.

**Algorithm:**

1. At decoration update time, allocate a single offscreen buffer sized to the
   full decoration frame (in buffer pixels = logical pixels, since
   buffer_scale = 1).

2. Render all decoration pieces (corners, sides, bottom, title, buttons) into
   this buffer using their logical coordinates. Since the buffer is at 1:1
   pixel scale and all coordinates are integers, there is no rounding.

3. Cut out the window content area (make it transparent) so the client surface
   shows through.

4. Present the buffer as a single `TextureRenderElement` with buffer_scale = 1.
   The compositor scales this single element to physical resolution, and any
   fractional rounding applies uniformly to the entire decoration frame rather
   than to individual pieces.

**Advantages:**
- Completely eliminates inter-element alignment issues at any scale factor.
- Simplifies the render element output (one element instead of ~15+).
- The tiling shader can be applied during the offscreen render pass using
  integer coordinates, avoiding fractional math entirely.

**Disadvantages:**
- Requires an offscreen render pass (FBO) every time decorations change (window
  resize, theme change, active/inactive state change, button hover).
- The offscreen buffer consumes GPU memory proportional to the decoration frame
  size.
- Damage tracking becomes coarser — any decoration change dirties the entire
  buffer. Could be mitigated by splitting into separate buffers (e.g., titlebar,
  left, right, bottom) but that partially re-introduces the alignment issue
  between the sub-buffers.
- More complex implementation: need to manage FBO lifecycle, render the tiling
  shader into the FBO, handle buffer resizing on window geometry changes.

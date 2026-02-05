# Initial Window Placement

These notes are somewhat disorganized, and are incomplete, but I want to
check them in now so I don't lose them.

Note: flags `PPosition` and `USPosition` aren't relevant on Wayland, as
neither the user nor program can specify window position.

## Algorithm

Somewhat simplified:

1. Find monitor at mouse pointer location.
2. If window is transient: center over parent; if constrained, do
   _something_ (TBD).
3. Figure out the largest possible size the window can be for the
   monitor, by taking into account the user-set monitor margins and any
   exclusive areas set by layer-shell surfaces (this size is called
   `full`). (On X11, this takes into account struts, but there are no
   struts on Wayland, just exclusive areas set by layer-shell surfaces.)
4. If window is not transient:
   - If smart placement ratio at or above max (100), or if `100*w*h` is
     less than `placement_ratio*full.w*full.h`:
     - If placement mode is mouse, do mouse placement.
     - Otherwise do center placement.
   - If frame extents are larger than `full` in either dimension, do center
     placement.
   - Otherwise do smart placement.
5. Ultimately if the window is too large for `full`, maximize it.

### Mouse placement

1. Place so center of window is under mouse pointer.
2. Ensure that the window is fully on-screen, and not inside margins or
   exclusive zones (move window around if this test fails).

### Center placement

1. Simple: place window in the center of the monitor.

### Smart placement

Hooo boy, this is complex.  Basically what it does is test various
positions, counting overlaps with other windows, until it finds the
position that causes the least overlap.  Probably in this case I should just
translate the C code into rust and not be too clever.

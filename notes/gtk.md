# Basic operation

Since xfwm4 uses GTK3 to draw some UI bits, xfwl4 does as well.
However, since the GTK3 main loop can't easily be integrated into
Calloop (well, ok, it sorta can, but it's not ideal, and not calling
`gtk_main()` has a few possibly-unwanted side-effects), all this has to
be done very carefully.

The Wayland display and smithay's work all happens on the main thread,
as seemingly makes sense.  GTK, however, runs on a separate thread.  It
turns out running GTK on another thread is a little tricky when the main
thread also needs to do things that involve a glib main loop.

GTK is a little rude and assumes that it can own and use the *default*
`GMainContext` (the main context is a per-thread thing that glib's main
loop uses).  Whatever code/thread first calls
`g_main_context_get_default()` essentially becomes its owner.

However, we do things on the main thread that require use of a glib main
context: namely, libxfconf makes D-Bus calls using GIO's D-Bus support,
which uses glib's main context.  Fortunately, GIO's D-Bus stuff will
first try to use something call the "thread default" main context,
before falling back to the default main context.

There's one other thing that needs to be accounted for: when you
initialize GTK, it will immediately try to open a display.  If that
fails, it throws an error.  We want it to open our new Wayland display,
of course, but we have to make sure we don't call `gtk_init()` until
that display is actually up and running (or, at least, is about to be
within a very, very short amount of time).

So to make this work, we need to do the following, in order:

1. Start up the GTK UI thread, and wait.
2. The GTK UI thread claims the main context by calling
   `g_main_context_get_default()`.  Then it signals the main thread that
   it's done so, and waits.
3. The main thread creates its own `GMainContext`, acquires it, and
   sets it as the thread-default main context.
4. The main thread then initializes xfconf, and sets up any D-Bus
   related things it needs to set up.
5. Once the Wayland display is ready, and the main thread is about to
   run its Calloop-based main loop, it signals the UI thread that it's
   about to do so.
6. The UI thread calls `gtk_init()` (which successfully connects to our
   new Wayland display), sets up whatever it needs to set up, and then
   calls `gtk_main()`.

# Presenting UI

Actually presenting the UI requires some extra work, especially since,
on Wayland, clients don't get to control fine details like where their
windows get placed.  In xfwl4, the UI thread, where GTK runs, doesn't
really know anything about the compositor, and behaves more or less like
any other Wayland client.

## Tabwin

The tab window shows up when the user presses Alt+Tab (or whatever
they've configured as the cycle-windows shortcut), in order to display a
grid/list of all windows, and allow switching between them.

The tabwin is implemented in `src/ui/tabwin.rs` as a `GtkWindow`
subclass.  It's triggered when the main thread (in its keyboard shortcut
handler) sends a message via a MPSC channel to a receiver registered
with the UI thread's `GMainContext`.  This message contains a list of
all windows, including title, preview image, icon, and whether or not
the window is minimized.

Once the tabwin comes up, the user can interact with it, but that
interaction is handled in a bit of a split manner.  Pointer movement is
handled just like in any other GTK app, with the GTK code connecting to
pointer-related signals on the window.  But since the compositor
controls the keyboard shortcut, the simplest (and safest) thing to do
when the user presses Tab (or Shift+Tab) successive times is for the
keyboard shortcut handler to send other signals to the UI thread, which
then get forwarded to the tabwin widget (things like "advance to next
window" and "cancel tabwin").  On successful selection, the UI thread
will send back the ID of the selected window, so the compositor can
raise and activate it.

The tabwin sets its window name to something that the compositor knows
about, so when a new window appears that was created by the UI thread's
client connection, the compositor can center it properly on-screen.

## Window Menu

The window menu is hard, and I haven't figured out how to get it to work
properly yet in some situations.

At any rate, it's also implemented using GTK on the UI thread, using a
`GtkMenu`.  On Wayland, popups *must* have a toplevel window as its
parent, so the UI thread creates a transparent window to use as the
menu's parent.

### `show_window_menu` request

The `xdg_toplevel` interface has a request called `show_window_menu`,
which lets a client ask the compositor to pop up the window menu, at
coordinates (relative to one of the client's windows) that the client
specifies.

I haven't been able to get this to work properly yet.  Popup
windows/menus are special because they need to grab the keyboard and
pointer, and when the pointer is already pressed (say, perhaps the
client sent the `show_window_menu` request when the user right-clicked
on the window's client-side decorations), GTK can get confused when you
map a window and pop up a menu on it.  The issue is that the original
pointer press event was never seen by GTK, and its internal state
doesn't really know what to do when a menu pops up and it thinks the
pointer is not pressed when it really is.  I feel like I've solved a
similar problem before (albeit on X11), so I think there must be a way
to make this work, but I haven't yet figured it out.

### Window menu from server-side decorations

If a window has server-side decorations, and the user has configured
window titlebars to have a window menu button, we need to pop up the
menu when the user clicks on the button.

This is significantly easier.  My plan here is that, for
server-side-decorated windows, I'll have the UI thread create a small,
transparent, undecorated toplevel window that is the size of the
titlebar's window menu button's clickable area.  Once it's done so, I'll
place it on top of the titlebar in the correct spot, taking care to move
it whenever the window is moved or resized.  This window will not accept
focus, but if the user clicks on it, GTK will get the event normally,
and pop up the menu.  I am pretty sure this will work fine.

# Theming

In order to draw server-side decorations, the compositor (usually) needs
to know some particular theme colors, which are dependent on the
user-selected GTK theme.  It will also need to load window icons, which
will sometimes come from the user-selected icon theme.

The GTK thread will track these theme colors, and on a change, send the
specific color values the compositor needs to the main thread.  Then the
compositor can use them when drawing decorations.

For the icon theme, GTK can tell the main thread the name of the current
icon theme, and notify the main thread of changes.  `GtkIconTheme`, if
used carefully, can be used standalone from the main thread, as long as:

1. It isn't instantiated until after the UI thread calls `gtk_init()`,
   and
2. We use `gtk_icon_theme_new()` and `gtk_icon_theme_set_custom_theme()`
   instead of `gtk_icon_theme_get_default()`, as the latter interacts
   with GDK and uses the `GdkScreen` instance, which is not done in a
   thread-safe way.

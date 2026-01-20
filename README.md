# xfwl4: Xfce's Wayland Compositor

xfwl4 will be Xfce's Wayland compositor.

It is currently heavily under development, and not at all particularly
functional.

**NB: I will sometimes rewrite history and force-push to this repo!**

## Building

xfwl4 is written in Rust, targeting compiler version 1.88.0 and above.
If your distro does not provide rustc/cargo packages, or provides
packages that are too old, you can use [rustup](https://rustup.rs/) to
install a current toolchain.

Additionally, you will need the following development packages installed
(names may differ depending on your distro):

* `libdisplay-info-dev`
* `libdrm-dev`
* `libgbm-dev`
* `libinput-dev`
* `libpixman-1-dev`
* `libseat-dev`
* `libudev-dev`
* `libxkbcommon-dev`
* `xwayland`

You may not need some of these if you disable some features of the
application.

If you are just doing development, you can use the regular `cargo`
commands directly to build, test, and run the project.  For "proper"
build and install, you can use meson:

To build and install a release-optimized version of xfwl4:

```bash
meson setup --wipe --buildtype=release build
meson compile -Cbuild
meson install -Cbuild
```

To build and install a debug build of xfwl4:

```bash
meson setup --wipe build
meson compile -Cbuild
meson install -Cbuild
```

See `meson_options.txt` for `-D` options you can pass to `meson setup`
to change what features are enabled in the project.

You can pass `--prefix`, `--bindir`, etc. to `meson setup`, and
`--destdir` to `meson install` to change install behavior.

## Running

You can simply run `xfwl4` in order to start the compositor.  If started
from a TTY, it will run as a full session on the TTY.  If started under
an existing Wayland or X11 session, it will run windowed inside your
existing session.

See `xfwl4 --help` for other options, and for how to override the
backend selection.

## Docker

The `Dockerfile` is not useful for regular use.  It's only there to
ensure the project builds properly on a "clean" system with a Rust
version matching the MSRV.

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

To build and install a release-optimized version of xfwl4:

```bash
make
make install
```

To build and install a debug build of xfwl4:

```bash
make build-dev
make install-dev
```

You can use the variables `DESTDIR`, `PREFIX`, `BINDIR`, `DATADIR`, etc.
to configure the installation location.  `PREFIX` is set to `/usr/local`
by default.

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

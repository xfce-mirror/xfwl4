# xfwl4: Xfce's Wayland Compositor

xfwl4 will be Xfce's Wayland compositor.

It is currently heavily under development.  While basic things (and some
not-so-basic things) work, many xfwm4 behaviors and settings are not
implemented yet, and there are definitely bugs.

**NB: I will sometimes rewrite history and force-push to this repo!**

## Building

### Prerequisites

xfwl4 is written in Rust, targeting compiler version 1.88.0 and above.
If your distro does not provide rustc/cargo packages, or provides
packages that are too old, you can use [rustup](https://rustup.rs/) to
install a current toolchain.

Additionally, you will need the following development packages installed
(names may differ depending on your distro):

* `libdisplay-info-dev`
* `libdrm-dev`
* `libgbm-dev`
* `libgtk-3-dev`
* `libinput-dev`
* `libpixman-1-dev`
* `libseat-dev`
* `libudev-dev`
* `libxfconf-0-dev`
* `libxkbcommon-dev`
* `meson`
* `xwayland`

You may not need some of these if you disable some features of the
application.

xfwl4 also requires xfwm4 to be installed at present, for the `defaults`
file and decoration themes.

### Building

#### Development

If you are just doing development or testing, you can use the regular
`cargo` commands (`cargo build`, `cargo test`, `cargo run` directly to
build, test, and run the project.

You will need to set `XFWM4_PKGDATADIR` in the environment when building
(and/or when using `cargo run`, if you choose not to run the built
executable directly) so it can find the `defaults` file.  For most
people, this will probably be `/usr/share/xfwm4`.  (You can also just
set `PREFIX` to the same prefix where xfwm4 is installed.)

You will need xfwm4's themes installed in one of the search paths
(`~/.themes`, `~/.local/share/themes`, `$XDG_DATA_DIRS/themes`) in order
for xfwl4 to find them.

#### Release/Install

For "proper" build and install, use meson:

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
to change what features are enabled in the project.  One option of note
is `-Dxfwm4-pkgdatadir`, which will need to be set to the location of
your xfwm4 install if you are building xfwl4 for a different install
prefix than xfwm4.

You can pass `--prefix`, `--bindir`, etc. to `meson setup`, and
`--destdir` to `meson install` to change install behavior.

xfwl4's build system does not use meson's built-in support for Rust;
instead it runs cargo directly in order to take advantage of cargo's
dependency resolution, and for fetching dependencies from `crates.io`.

## Running

If you're running from the source tree, you can use `cargo run`, or run
`target/debug/xfwl4` (or `target/release/xfwl4`) directly.  (Remember
that for `cargo run`, you may need to set `XFWM4_PKGDATADIR` in the
environment first).

If you've installed via meson to somewhere in your `PATH`, you can
simply run `xfwl4` in order to start the compositor.

If started from a TTY, it will run as a full session on the TTY, and it
will update the systemd user session's (if any) and dbus session bus's
activation environment to reflect the Wayland session.  If you don't
want it to do this (perhaps you are running an X11 session on another
TTY and are just playing around with xfwl4), pass `--no-session` to
xfwl4.  In this mode, it will also not start any desktop services like
`xfsettingsd` or `xfce4-session` (well, currently there is *no* mode
where it will run those programs, but eventually it will).

If started under an existing Wayland or X11 session, it will run
windowed inside your existing session, as a standalone compositor
without touching your user session.

See `xfwl4 --help` for other options, and for how to override the
backend selection.

## Docker

`Dockerfile.cargo` and `Dockerfile.meson` are not useful for regular
use.  They're only there to ensure the project builds properly on a
"clean" system with a Rust version matching the MSRV.

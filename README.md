# xfwl4: Xfce's Wayland Compositor

xfwl4 will be Xfce's Wayland compositor.

It is currently heavily under development.  While many xfwm4 (and other
desktop environment) features are implemented, some things are *not*
implemented yet, and I'm sure there are bugs in what's there.

**NB: I will sometimes rewrite history and force-push to this repo!**

## Building

### Prerequisites

xfwl4 is written in Rust, targeting compiler version 1.90.0 and above.
If your distro does not provide rustc/cargo packages, or provides
packages that are too old, you can use [rustup](https://rustup.rs/) to
install a current toolchain.

Additionally, you will need the following development packages installed
(names may differ depending on your distro):

* `gettext`
* `libdisplay-info-dev` >= 0.3.0
* `libdrm-dev`
* `libgbm-dev`
* `libgtk-3-dev`
* `libinput-dev` >= 1.28.0
* `libpixman-1-dev`
* `libseat-dev`
* `libudev-dev`
* `libxfce4ui-2-0` >= 4.21.4
* `libxfconf-0-dev` >= 4.21.2
* `libxkbcommon-dev`
* `meson` >= 0.57.0
* `xwayland`

You may not need some of these if you disable some features of the
application.

You will also need these versions of the following Xfce components
installed:

* `xfwm4` (any version)
    * `xfwl4` uses `xfwm4`'s themes and will not start without them.
      You can also use `xfwm4`'s settings dialogs to configure `xfwl4`'s
      window management behavior.
* `xfce4-settings` (git rev `cf707871` or newer)
    * Wayland-related fixes for Display and Keyboard settings.  Note
      that Mouse/Touchpad settings does not yet work on Wayland.
* `xfdesktop` (git rev `5756e94d` or newer)
    * Fixes for the settings dialog while running under Wayland.

### Building

The repository makes use of git submodules; make sure you clone with
`--recursive`, or, if you already have a checkout without the
submodules, run `git submodule update --init`.

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
to change what features are enabled in the project.

You can pass `--prefix`, `--bindir`, etc. to `meson setup`, and
`--destdir` to `meson install` to change install behavior.

xfwl4's build system does not use meson's built-in support for Rust;
instead it runs cargo directly in order to take advantage of cargo's
dependency resolution, and for fetching dependencies from `crates.io`.

#### Development

If you are just doing development or testing, you can use the regular
`cargo` commands (`cargo build`, `cargo test`, `cargo run` directly to
build, test, and run the project.

You will need xfwm4's themes installed in one of the search paths
(`~/.themes`, `~/.local/share/themes`, `$XDG_DATA_DIRS/themes`) in order
for xfwl4 to find them.

If you plan to contribute, read the [contributing
document](CONTRIBUTING.md) first.

## Running

If you're running from the source tree, you can use `cargo run`, or run
`target/debug/xfwl4` (or `target/release/xfwl4`) directly.

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
without touching your user session.  Note that I barely ever run it this
way, so it probably doesn't work well, and I'm not prioritizing issues
with nested mode.

See `xfwl4 --help` for other options, and for how to override the
backend selection.

## Docker

`Dockerfile.cargo` and `Dockerfile.meson` are not useful for regular
use.  They're only there to ensure the project builds properly on a
"clean" system with a Rust version matching the MSRV.

## Further Information

See the [xfwl4 FAQ](https://wiki.xfce.org/xfwl4_faq) or visit us on
[Matrix](https://matrix.to/#/#xfce-dev:matrix.org).  Note that there is
no user support at this time; you are on your own getting things working
until there is a published release.

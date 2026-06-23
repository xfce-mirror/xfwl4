project(
  'xfwl4',
  'rust',
  # Annoyingly, b_lto (and possibly b_ndebug) requires C to be enabled for the
  # option values to actuall propagate to this script.
  'c',
version : '4.21.0-dev',
  license : 'GPL-3.0-or-later',
  meson_version : '>= 0.57.0',
  default_options : ['buildtype=debugoptimized', 'warning_level=2']
)

gnome = import('gnome')
i18n = import('i18n')

dependency_versions = {
  'glib': '>= 2.72.0',
  'gtk': '>= 3.24.0',
  'libdisplay-info': '>= 0.3.0',
  'libdrm': '>= 2.4.0',
  'libgbm': '>= 25.0.0',
  'libinput': '>= 1.28.0',
  'libpixman': '>= 0.44.0',
  'libseat': '>= 0.9.0',
  'libudev': '>= 257',
  'libxfce4kbd-private': '>= 4.21.4',
  'libxfconf': '>= 4.21.2',
  'libxkbcommon': '>= 0.8.0',
  'rust': '>= 1.90.0',
}

cargo = find_program('cargo', version: dependency_versions['rust'])
rustc = find_program('rustc', version: dependency_versions['rust'])

glib = dependency('glib-2.0', version: dependency_versions['glib'])
gtk = dependency('gtk+-3.0', version: dependency_versions['gtk'])
libxfce4kbd_private = dependency('libxfce4kbd-private-3', version: dependency_versions['libxfce4kbd-private'])
xfconf = dependency('libxfconf-0', version: dependency_versions['libxfconf'])

libdisplay_info = dependency('libdisplay-info', version: dependency_versions['libdisplay-info'])
libdrm = dependency('libdrm', version: dependency_versions['libdrm'])
libgbm = dependency('gbm', version: dependency_versions['libgbm'])
libinput = dependency('libinput', version: dependency_versions['libinput'])
libpixman = dependency('pixman-1', version: dependency_versions['libpixman'])
libseat = dependency('libseat', version: dependency_versions['libseat'])
libudev = dependency('libudev', version: dependency_versions['libudev'])
libxkbcommon = dependency('xkbcommon', version: dependency_versions['libxkbcommon'])

install = find_program('install')

cargo_build_test_options = [
  '--manifest-path', meson.current_source_dir() / 'Cargo.toml',
  '--target-dir', '@0@/xfwl4'.format(meson.current_build_dir()),
  '--no-default-features',
]

optimization = get_option('optimization')
opt_level = ''
if optimization in ['0', '1', '2', '3']
  opt_level = optimization
elif optimization == 's'
  opt_level = '"s"'
elif optimization == 'g'
  opt_level = '1'
elif optimization != 'plain'
  opt_level = '2'
endif

if opt_level != ''
  cargo_build_test_options += ['--config', 'profile.dev.opt-level=@0@'.format(opt_level)]
endif

debuginfo = ''
if get_option('debug')
  debuginfo = '"full"'
else
  debuginfo = '"none"'
endif

if debuginfo != ''
  cargo_build_test_options += ['--config', 'profile.dev.debug=@0@'.format(debuginfo)]
endif

buildtype = get_option('buildtype')
if buildtype in ['release', 'plain']
  cargo_build_test_options += ['--config', 'profile.dev.codegen-units=16']
endif

b_ndebug = get_option('b_ndebug')
ndebug = b_ndebug == 'true' or (b_ndebug == 'if-release' and buildtype in ['release', 'plain'])
if ndebug
  cargo_build_test_options += ['--config', 'profile.dev.debug-assertions=false']
  cargo_build_test_options += ['--config', 'profile.dev.overflow-checks=false']
endif

lto = ''
if get_option('b_lto')
  if get_option('b_lto_mode') == 'thin'
    lto = '"thin"'
  else
    lto = '"fat"'
  endif
else
  lto = 'false'
endif
cargo_build_test_options += ['--config', 'profile.dev.lto=@0@'.format(lto)]

if get_option('strip')
  cargo_build_test_options += ['--config', 'profile.dev.strip=true']
endif

backends = get_option('backends')
if 'udev' in backends
  cargo_build_test_options += ['-F', 'udev']
endif
if 'winit' in backends
  cargo_build_test_options += ['-F', 'winit']
endif
if 'x11' in backends
  cargo_build_test_options += ['-F', 'x11']
endif

if get_option('egl')
  cargo_build_test_options += ['-F', 'egl']
endif

if get_option('xwayland')
  cargo_build_test_options += ['-F', 'xwayland']
endif

if get_option('use-system-gettext')
  cargo_build_test_options += ['-F', 'gettext-system']
endif

if get_option('debug-rendering')
  cargo_build_test_options += ['-F', 'debug']
endif

env = {
  'PREFIX': get_option('prefix'),
  'BINDIR': get_option('prefix') / get_option('bindir'),
  'DATADIR': get_option('prefix') / get_option('datadir'),
  'PKGDATADIR': get_option('prefix') / get_option('datadir') / 'xfce4' / 'xfwl4',
  'LOCALEDIR': get_option('prefix') / get_option('datadir') / 'locale',
}

xfwl4 = custom_target(
  'xfwl4',
  output: 'xfwl4',
  env: env,
  command: [cargo, 'build'] + cargo_build_test_options,
  console: true,
  build_by_default: true,
  build_always_stale: true,
)
# Can't use custom_target()'s installation feature, as it will install
# _everything_, including all the rust intermediate build files.
meson.add_install_script(
  'meson.install',
  install,
  meson.current_build_dir() / 'xfwl4' / 'debug' / 'xfwl4',
  get_option('bindir'),
)

test(
  'xfwl4-test',
  cargo,
  args: ['test'] + cargo_build_test_options,
  env: env,
  depends: xfwl4,
  protocol: 'exitcode',
  # Bump this if builds fail due to timeout errors.
  timeout: 600,
)

subdir('po')
subdir('resources')

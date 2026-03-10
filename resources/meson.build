wayland_session_prefix = get_option('wayland-session-prefix')
if wayland_session_prefix == ''
  wayland_session_prefix = '/usr'
endif

i18n.merge_file(
  input: 'xfwl4.desktop.in',
  output: 'xfwl4.desktop',
  po_dir: 'po',
  type: 'desktop',
  install: true,
  install_dir: wayland_session_prefix / get_option('datadir') / 'wayland-sessions',
)

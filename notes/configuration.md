xfwm4's configuration uses a layered approach:

1. First, default values are read from the `$PKGDATADIR/defaults` file.
   This is a simple `key=value` style configuration file.
2. Next, we try to load (almost) every setting from xfconf as well,
   prefixing `/general/` to the configuration key.  These values
   override the defaults loaded in the previous step.
3. Finally, we look at the `theme` setting key, take its value, and
   attempt to load the theme from
   `$XDG_DATA_DIRS/themes/$THEME_NAME/xfwm4/themerc`.  This is also a
   `key=value` configuration file, and values here can also override
   anything set in `defaults` or in xfconf.

When the `theme` changes, we unload everything and start over.

This is implemented in:

* `src/util/rc.rs`
* `src/config/xfwl4_config.rs`
* `src/config/xfwl4_config_types.rs`

Settings values can be strings, ints (`i32`), or booleans.
Additionally, strings can be colors, where the value is either a
HTML-style `#rrggbbaa` value (alpha is optional), or a named color that
refers to an XPM symbolic color name or a GTK theme color name.  All of
this is implemented in `rc.rs`.

Some strings are enum values; the enums are defined and parsing
implemented in `xfwl4_config_types.rs`.

Finally, `xfwl4_config.rs` implements the `Xfwl4Config` struct, and uses
the machinery in `rc.rs` and xfconf in order to load and maintain the
configuration.

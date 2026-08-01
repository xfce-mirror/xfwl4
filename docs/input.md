# Input Configuration

xfwl4 (as well as most Wayland compositors) use libinput to handle
talking to keyboards, mice, touchpads, tablets, and touchscreens.

## Keyboard

The Keyboard Settings dialog handles keyboard configuration.  You can
set your keyboard model, layouts, and options there, as well as key
repeat delay and speed.

### Xfconf Schema

Settings are stored in xfconf, in the `keyboard-layout` and `keyboards`
channel.  Below are the properties read by xfwl4.

#### `keyboards`

| Property | Type | Notes |
| :------: | :--: | :---: |
| `/Default/KeyRepeat` | bool | Enable/disable for key repeat |
| `/Default/KeyRepeat/Delay` | int | Delay before key repeat starts, in milliseconds |
| `/Default/KeyRepeat/Rate` | int | Key repeat rate, in repeats per second |
| `/Default/RestoreNumlock` | bool | Whether or not to restore the previous state of the Num Lock key on startup |

The `/Default/XkbDisable` option is ignored by xfwl4.
`/Default/Numlock` is used to store the current/last Num Lock state so
it can be restored if `/Default/RestoreNumlock` is enabled.

#### `keyboard-layouts`

| Property | Type | Notes |
| :------: | :--: | :---: |
| `/Default/XkbLayout` | string | A comma-separated list of layout names (xfwl4 currently only activates the first in the list) |
| `/Default/XkbModel` | string | Keyboard model identifier |
| `/Default/XkbOptions/Compose` | Compose key |
| `/Default/XkbOptions/Group` | Layout group option |
| `/Default/XkbVariant` | A comma-separated list of layout variant names, in the same order as `XkbLayout` above |

### Hidden Settings

If you'd like to set more XKB-style options on the keyboard that aren't
supported by the dialog, you can create a uniquely named xfconf property
in the `keyboard-layout` channel, under the `/Default/XkbOptions/`
hierarchy.  For example, if I want to remap my Caps Lock key to be an
Escape key, I can do:

```
xfconf-query \
    --channel keyboard-layout \
    --property /Default/XkbOptions/CapsLock \
    --create \
    --type string \
    --set "caps:escape"
```

Note that there is no significance to choosing `CapsLock` for the
property name; just make sure it's something unique.  The settings
dialog writes to `Compose` and `Group` already, so don't use them for
something else.

As for the values, you'll need to find some XKB documentation that lists
what's valid.

## Mouse, Touchpad, Tablet, and Touchscreen

Currently xfwl4 only supports a small number of settings for pointer
devices, configured through the Mouse and Touchpad Settings dialog.

### Tablets

Tablet support has not been tested, and configuration options for it are
not exposed.  The basic plumbing is there, so it may work, but probably
not well.

### Touchscreens

Touchscreen support has been tested somewhat, and configuration for
assigning a monitor, and rotating/reflecting the input events is
available.  However, some UI elements may not respond to touch as
expected.

### Xfconf Schema

Settings are stored in xfconf in the `pointers` channel.  Settings are
per-device, and the property format is `/$DEVICE_NAME/$SETTING_NAME`.
Device names are (on Linux) the kernel evdev name.  You can see
`/sys/class/input/event*/device/name` for a list of names on your
system, or use the `libinput list-devices` command (must be run as root)
to enumerate devices and their capabilities.

We take the device name and transform it before using it in the property
name.  ASCII alphanumeric characters, plus `-` and `_` are kept as is,
and spaces are converted to underscores.  All other characters are
dropped.

The `$SETTING_NAME` portion of the property names are as follows.  Note
that not all settings are available for all device types.

| Property | Type | Device Type | Notes |
| :------: | :--: | :---------: | :---: |
| `/Properties/Device_Enabled` | int | all | Enable/disable the device |
| `/Properties/libinput_Accel_Speed` | float | mouse, touchpad | Acceleration speed |
| `/Properties/libinput_Accel_Profile_Enabled` | array of 3 ints | mouse, touchpad | Acceleration profiles enabled, \[adaptive, flat, custom]; custom is not supported |
| `/Properties/libinput_Click_Method_Enabled` | array of 2 ints | touchpad | Click methods, \[edges, clickfinger] |
| `/Properties/libinput_Disable_While_Typing_Enabled` | int | touchpad | 1 to disable the touchpad while typing |
| `/Properties/libinput_Left_Handed_Enabled` | int | mouse, touchpad | 1 to swap left/right mouse buttons |
| `/Properties/libinput_Natural_Scrolling_Enabled` | int | mouse with wheel, touchpad | 1 to reverse scrolling direction |
| `/Properties/libinput_Scroll_Method_Enabled` | array of 3 ints | touchpad | Scroll method, \[twofinger, edge, buttondown]; buttondown is not supported |
| `/Properties/libinput_Tapping_Enabled` | int | touchpad | 1 to use tap as primary button press |
| `/Properties/Wacom_Rotation` | int | tablet | 0=normal, 1=90 degrees, 2=270 degrees, 3=180 degrees |
| `/AssignedMonitor` | string | tablet, touchscreen | sha1 hash of the EDID of the monitor the touchscreen is attached to |
| `/Reflection` | string | tablet, touchscreen | "0", "X", "Y", or "XY" |
| `/Rotation` | int | tablet, touchscreen | degrees to rotate |

Additionally, the following properties are recognized for
backwards-compatibility, but should not be used going forward:

* `/Acceleration` (use `/Properties/libinput_Accel_Speed`)
* `/ReverseScrolling` (use `/Properties/libinput_Natural_Scrolling_Enabled`)
* `/RightHanded` (use `/Properties/libinput_Left_Handed_Enabled`)
* `/Properties/Synaptics_Tap_Action` (use `/Properties/libinput_Tapping_Enabled`)
* `/Properties/Synaptics_Edge_Scrolling` (use `/Properties/libinput_Scroll_Method_Enabled`)
* `/Properties/Synaptics_Two-Finger_Scrolling` (use `/Properties/libinput_Scroll_Method_Enabled`)

The following properties are not supported:

* `/Mode` (libinput does not support changing a device between absolute
  and relative mode)
* `/Properties/Synaptics_Circular_Scrolling_Trigger` (libinput does not
  support this)
* `/Properties/Synaptics_Circular_Scrolling` (libinput does not support
  this)
* `/Properties/libinput_High_Resolution_Wheel_Scroll_Enabled` (this is
  always enabled and cannot be disabled)
* `/Threshold` (libinput does not support this)

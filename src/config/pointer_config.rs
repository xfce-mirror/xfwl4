// xfwl4 -- Wayland compositor for the Xfce Desktop Environment
//
// Copyright (C) 2026 Brian Tarricone <brian@tarricone.org>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Context, anyhow};
use smithay::reexports::input::{ClickMethod, Device, ScrollMethod};
use xfconf::{Array, ChannelExtManual};

use crate::config::POINTERS_CHANNEL_NAME;

const PROP_ACCELERATION: &str = "/Acceleration";
const PROP_REVERSE_SCROLLING: &str = "/ReverseScrolling";
const PROP_RIGHT_HANDED: &str = "/RightHanded";
const PROP_THRESHOLD: &str = "/Threshold";
const PROP_DEVICE_ENABLED: &str = "/Properties/Device_Enabled";
const PROP_LIBINPUT_ACCEL_SPEED: &str = "/Properties/libinput_Accel_Speed";
const PROP_LIBINPUT_ACCEL_PROFILE_ENABLED: &str = "/Properties/libinput_Accel_Profile_Enabled";
const PROP_LIBINPUT_ACCEL_PROFILES_AVAILABLE: &str = "/Properties/libinput_Accel_Profiles_Available";
const PROP_LIBINPUT_CLICK_METHOD_ENABLED: &str = "/Properties/libinput_Click_Method_Enabled";
const PROP_LIBINPUT_CLICK_METHODS_AVAILABLE: &str = "/Properties/libinput_Click_Methods_Available";
const PROP_LIBINPUT_DISABLE_WHILE_TYPING_ENABLED: &str = "/Properties/libinput_Disable_While_Typing_Enabled";
const PROP_LIBINPUT_HIGH_RESOLUTION_WHEEL_SCROLL_ENABLED: &str = "/Properties/libinput_High_Resolution_Wheel_Scroll_Enabled";
const PROP_LIBINPUT_LEFT_HANDED_ENABLED: &str = "/Properties/libinput_Left_Handed_Enabled";
const PROP_LIBINPUT_NATURAL_SCROLLING_ENABLED: &str = "/Properties/libinput_Natural_Scrolling_Enabled";
const PROP_LIBINPUT_SCROLL_METHOD_ENABLED: &str = "/Properties/libinput_Scroll_Method_Enabled";
const PROP_LIBINPUT_SCROLL_METHODS_AVAILABLE: &str = "/Properties/libinput_Scroll_Methods_Available";
const PROP_LIBINPUT_TAPPING_ENABLED: &str = "/Properties/libinput_Tapping_Enabled";
const PROP_SYNAPTICS_TAP_ACTION: &str = "/Properties/Synaptics_Tap_Action";
const PROP_SYNAPTICS_EDGE_SCROLLING: &str = "/Properties/Synaptics_Edge_Scrolling";
const PROP_SYNAPTICS_TWO_FINGER_SCROLLING: &str = "/Properties/Synaptics_Two-Finger_Scrolling";
const PROP_SYNAPTICS_CIRCULAR_SCROLLING: &str = "/Properties/Synaptics_Circular_Scrolling";
const PROP_SYNAPTICS_CIRCULAR_SCROLLING_TRIGGER: &str = "/Properties/Synaptics_Circular_Scrolling_Trigger";
const PROP_WACOM_ROTATION: &str = "/Properties/Wacom_Rotation";
const PROP_TABLET_MODE: &str = "/Mode";

#[derive(Debug, Clone)]
#[allow(dead_code)] // We need to hold the channel alive, but never need to use it
pub struct PointerConfig(xfconf::Channel);

impl PointerConfig {
    pub fn new(mut device: Device) -> Self {
        tracing::info!("Configuring new pointer: {}", device.name());

        let property_base = format!("/{}", device_name_to_xfconf_name(device.name()));
        let channel = xfconf::Channel::with_property_base(POINTERS_CHANNEL_NAME, &property_base);

        channel.connect_property_changed(None, {
            let device = device.clone();
            let channel = channel.clone();
            move |_, property_name, value| {
                let mut device = device.clone();
                Self::handle_property_changed(&channel, &mut device, property_name, value);
            }
        });

        for (property_name, value) in channel.get_properties(None) {
            // The property-changed signal emission give us property names with the property base
            // removed, but .get_properties() includes the full property names.
            let property_name: String = property_name.as_str().chars().skip(property_base.len()).collect();
            Self::handle_property_changed(&channel, &mut device, &property_name, &value);
        }

        Self(channel)
    }

    fn handle_property_changed(channel: &xfconf::Channel, device: &mut Device, property_name: &str, value: &glib::Value) {
        fn handle(channel: &xfconf::Channel, device: &mut Device, property_name: &str, value: &glib::Value) -> anyhow::Result<()> {
            match property_name {
                PROP_ACCELERATION | PROP_LIBINPUT_ACCEL_SPEED => {
                    if property_name == PROP_ACCELERATION && channel.has_property(PROP_LIBINPUT_ACCEL_SPEED) {
                        // Prefer the libinput setting.
                        Ok(())
                    } else {
                        let acceleration = value
                            .get::<f64>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        // XInput acceleration value needs to be scaled for libinput.
                        let acceleration = ((acceleration / 5.) - 1.).clamp(-1., 1.);
                        tracing::debug!("Setting {} accel speed to {}", device.name(), acceleration);
                        device
                            .config_accel_set_speed(acceleration)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_REVERSE_SCROLLING => {
                    let reverse = value
                        .get::<bool>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} natural scroll to {}", device.name(), reverse);
                    device
                        .config_scroll_set_natural_scroll_enabled(reverse)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_RIGHT_HANDED => {
                    let right_handed = value
                        .get::<bool>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} left-handed to {}", device.name(), !right_handed);
                    device
                        .config_left_handed_set(!right_handed)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_THRESHOLD => {
                    // There doesn't seem to be an equivalent for this with libinput.
                    Ok(())
                }

                PROP_DEVICE_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    use smithay::reexports::input::SendEventsMode;
                    let mode = if enabled != 0 {
                        SendEventsMode::ENABLED
                    } else {
                        SendEventsMode::DISABLED
                    };
                    tracing::debug!("Setting {} send events mode to {:?}", device.name(), mode);
                    device
                        .config_send_events_set_mode(mode)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_ACCEL_PROFILE_ENABLED => {
                    let profile_arr = value
                        .get::<Array<i32>>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    use smithay::reexports::input::AccelProfile;
                    let mut iter = profile_arr.iter();
                    let flat = iter.next();
                    let adaptive = iter.next();
                    let profile = if let Some(flat) = flat
                        && *flat == 1
                    {
                        Some(AccelProfile::Flat)
                    } else if let Some(adaptive) = adaptive
                        && *adaptive == 1
                    {
                        Some(AccelProfile::Adaptive)
                    } else {
                        None
                    };
                    profile.map_or(Ok(()), |p| {
                        tracing::debug!("Setting {} accel profile to {:?}", device.name(), p);
                        device
                            .config_accel_set_profile(p)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    })
                }

                PROP_LIBINPUT_ACCEL_PROFILES_AVAILABLE => Ok(()),

                PROP_LIBINPUT_CLICK_METHOD_ENABLED => {
                    let method_arr = value
                        .get::<Array<i32>>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    let mut iter = method_arr.iter();
                    let areas = iter.next();
                    let fingers = iter.next();
                    let method = if let Some(areas) = areas
                        && *areas == 1
                    {
                        Some(ClickMethod::ButtonAreas)
                    } else if let Some(fingers) = fingers
                        && *fingers == 1
                    {
                        Some(ClickMethod::Clickfinger)
                    } else {
                        None
                    };
                    method.map_or(Ok(()), |m| {
                        tracing::debug!("Setting {} click method to {:?}", device.name(), m);
                        device
                            .config_click_set_method(m)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    })
                }

                PROP_LIBINPUT_CLICK_METHODS_AVAILABLE => Ok(()),

                PROP_LIBINPUT_DISABLE_WHILE_TYPING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} disable-while-typing to {}", device.name(), enabled != 0);
                    device
                        .config_dwt_set_enabled(enabled != 0)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_HIGH_RESOLUTION_WHEEL_SCROLL_ENABLED => {
                    // I thought there was a way to set this in libinput, but I can't find it.
                    // From what I can tell, for Wayland you need to edit some file under
                    // /etc/libinput, which is pretty lame.
                    Ok(())
                }

                PROP_LIBINPUT_LEFT_HANDED_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} left-handed to {}", device.name(), enabled != 0);
                    device
                        .config_left_handed_set(enabled != 0)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_NATURAL_SCROLLING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} natural scroll to {}", device.name(), enabled != 0);
                    device
                        .config_scroll_set_natural_scroll_enabled(enabled != 0)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_SCROLL_METHOD_ENABLED => {
                    let method_arr = value
                        .get::<Array<i32>>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    let mut iter = method_arr.iter();
                    let two_finger = iter.next();
                    let edge = iter.next();
                    let method = if let Some(two_finger) = two_finger
                        && *two_finger == 1
                    {
                        ScrollMethod::TwoFinger
                    } else if let Some(edge) = edge
                        && *edge == 1
                    {
                        ScrollMethod::Edge
                    } else {
                        ScrollMethod::NoScroll
                    };
                    tracing::debug!("Setting {} scroll method to {:?}", device.name(), method);
                    device
                        .config_scroll_set_method(method)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_SCROLL_METHODS_AVAILABLE => Ok(()),

                PROP_LIBINPUT_TAPPING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} tapping to {}", device.name(), enabled != 0);
                    device
                        .config_tap_set_enabled(enabled != 0)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_SYNAPTICS_TAP_ACTION => {
                    if channel.has_property(PROP_LIBINPUT_TAPPING_ENABLED) {
                        // Prefer the libinput setting
                        Ok(())
                    } else {
                        let tap_action = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = tap_action.iter().any(|&v| v != 0);
                        tracing::debug!("Setting {} tapping to {}", device.name(), enabled);
                        device
                            .config_tap_set_enabled(enabled)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_EDGE_SCROLLING => {
                    if channel.has_property(PROP_LIBINPUT_SCROLL_METHOD_ENABLED) {
                        // Prefer the libinput setting
                        Ok(())
                    } else {
                        let edge_scrolling = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = edge_scrolling.iter().any(|&v| v != 0);
                        let method = if enabled { ScrollMethod::Edge } else { ScrollMethod::NoScroll };
                        tracing::debug!("Setting {} scroll method to {:?}", device.name(), method);
                        device
                            .config_scroll_set_method(method)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_TWO_FINGER_SCROLLING => {
                    if channel.has_property(PROP_LIBINPUT_SCROLL_METHOD_ENABLED) {
                        // Prefer the libinput setting
                        Ok(())
                    } else {
                        let two_finger = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = two_finger.iter().any(|&v| v != 0);
                        let method = if enabled { ScrollMethod::TwoFinger } else { ScrollMethod::NoScroll };
                        tracing::debug!("Setting {} scroll method to {:?}", device.name(), method);
                        device
                            .config_scroll_set_method(method)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_CIRCULAR_SCROLLING | PROP_SYNAPTICS_CIRCULAR_SCROLLING_TRIGGER => {
                    // Libinput does not implement the Synaptics circular scrolling feature.
                    Ok(())
                }

                PROP_WACOM_ROTATION => {
                    let rotation = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    let angle = (rotation * 90).clamp(0, 270) as u32;
                    tracing::debug!("Setting {} rotation to {}°", device.name(), angle);
                    device
                        .config_rotation_set_angle(angle)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_TABLET_MODE => {
                    // Overriding absolute/relative mode is not supported with libinput.
                    Ok(())
                }

                name => Err(anyhow!("Unhandled pointer settings property {name} for device {}", device.name())),
            }
        }

        if let Err(err) = handle(channel, device, property_name, value) {
            tracing::warn!("{err}");
        }
    }
}

fn device_name_to_xfconf_name(name: &str) -> String {
    name.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                Some(c)
            } else if c == ' ' {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}

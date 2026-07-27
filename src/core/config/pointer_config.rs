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
use smithay::reexports::{
    calloop::{LoopHandle, RegistrationToken},
    input::{ClickMethod, Device, ScrollMethod},
};
use xfconf::{Array, ChannelExtManual};

use crate::{
    backend::udev::UdevData,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const POINTERS_CHANNEL_NAME: &str = "pointers";

const PROP_ACCELERATION: &str = "/Acceleration";
const PROP_REFLECTION: &str = "/Reflection";
const PROP_REVERSE_SCROLLING: &str = "/ReverseScrolling";
const PROP_RIGHT_HANDED: &str = "/RightHanded";
const PROP_ROTATION: &str = "/Rotation";
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

#[derive(Debug)]
pub struct PointerConfig {
    channel: xfconf::Channel,
    device: Device,
    source_token: Option<RegistrationToken>,
}

impl PointerConfig {
    pub fn new(device: Device, handle: LoopHandle<'_, Xfwl4State<UdevData>>) -> Self {
        tracing::info!("Configuring new pointer: {}", device.name());

        let property_base = format!("/{}", device_name_to_xfconf_name(&device.name()));
        let channel = xfconf::Channel::with_property_base(POINTERS_CHANNEL_NAME, &property_base);

        let source = CalloopXfconfSource::new(channel.clone(), []);
        let token = handle
            .insert_source(source, {
                let device_name = device.name().into_owned();
                move |(property_name, value), _, state| {
                    let changed_device = if let Some(config) = state.backend.pointer_config_by_name(&device_name)
                        && config.handle_property_changed(&property_name, value)
                    {
                        Some(config.device.clone())
                    } else {
                        None
                    };

                    if let Some(device) = changed_device {
                        state.backend.input_device_changed(&device);
                    }
                }
            })
            .expect("failed to insert xfconf source for pointer device");

        let mut config = Self {
            channel: channel.clone(),
            device,
            source_token: Some(token),
        };

        for (property_name, value) in channel.get_properties(None) {
            // The property-changed signal emission give us property names with the property base
            // removed, but .get_properties() includes the full property names.
            let property_name: String = property_name.as_str().chars().skip(property_base.len()).collect();
            config.handle_property_changed(&property_name, value);
        }

        config
    }

    pub fn shutdown(mut self) -> RegistrationToken {
        // .unwrap() is safe here because this function takes ownership of the object and drops it.
        self.source_token.take().unwrap()
    }

    fn handle_property_changed(&mut self, property_name: &str, value: glib::Value) -> bool {
        fn handle(channel: &xfconf::Channel, device: &mut Device, property_name: &str, value: glib::Value) -> anyhow::Result<bool> {
            match property_name {
                PROP_ACCELERATION => {
                    if channel.has_property(PROP_LIBINPUT_ACCEL_SPEED) {
                        // Prefer the libinput setting.
                        Ok(false)
                    } else {
                        let acceleration = value
                            .get::<f64>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let speed = if acceleration < 0. {
                            // The settings dialog stores a negative value to mean "unset".
                            device.config_accel_default_speed()
                        } else {
                            // XInput acceleration value needs to be scaled for libinput.
                            ((acceleration / 5.) - 1.).clamp(-1., 1.)
                        };
                        tracing::debug!("Setting {} accel speed to {}", device.name(), speed);
                        device
                            .config_accel_set_speed(speed)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_LIBINPUT_ACCEL_SPEED => {
                    // Unlike /Acceleration, this key holds a value in libinput's own [-1,1] range.
                    let speed = value
                        .get::<f64>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?
                        .clamp(-1., 1.);
                    tracing::debug!("Setting {} accel speed to {}", device.name(), speed);
                    device
                        .config_accel_set_speed(speed)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_REVERSE_SCROLLING => {
                    let reverse = value
                        .get::<bool>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} natural scroll to {}", device.name(), reverse);
                    device
                        .config_scroll_set_natural_scroll_enabled(reverse)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_RIGHT_HANDED => {
                    let right_handed = value
                        .get::<bool>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} left-handed to {}", device.name(), !right_handed);
                    device
                        .config_left_handed_set(!right_handed)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_ROTATION | PROP_REFLECTION | PROP_WACOM_ROTATION => {
                    if device.config_calibration_has_matrix() {
                        // These settings are expressed as a single libinput calibration matrix, so
                        // the ones that didn't change have to be read back from the channel.
                        let property = |name: &str| {
                            if name == property_name {
                                Some(value.clone())
                            } else {
                                channel.get_property_value(name)
                            }
                        };
                        let int_property = |name: &str| property(name).and_then(|value| value.get::<i32>().ok());

                        // /Rotation is in degrees and wins over the wacom driver's rotation enum.
                        let rotation = round_rotation_degrees(
                            int_property(PROP_ROTATION)
                                .or_else(|| int_property(PROP_WACOM_ROTATION).map(wacom_rotation_to_degrees))
                                .unwrap_or(0),
                        );
                        let reflection = property(PROP_REFLECTION)
                            .and_then(|value| value.get::<String>().ok())
                            .unwrap_or_default();

                        tracing::debug!(
                            "Setting {} calibration to rotation {}°, reflection '{}'",
                            device.name(),
                            rotation,
                            reflection
                        );
                        device
                            .config_calibration_set_matrix(calibration_matrix(rotation, &reflection))
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    } else {
                        tracing::debug!(
                            "Ignoring property '{property_name}' for {}, which has no calibration matrix",
                            device.name()
                        );
                        Ok(false)
                    }
                }

                PROP_THRESHOLD => {
                    // There doesn't seem to be an equivalent for this with libinput.
                    Ok(false)
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
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_ACCEL_PROFILE_ENABLED => {
                    let profile_arr = value
                        .get::<Array<i32>>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    use smithay::reexports::input::AccelProfile;
                    let mut iter = profile_arr.iter();
                    let adaptive = iter.next();
                    let flat = iter.next();
                    let profile = if let Some(adaptive) = adaptive
                        && *adaptive == 1
                    {
                        Some(AccelProfile::Adaptive)
                    } else if let Some(flat) = flat
                        && *flat == 1
                    {
                        Some(AccelProfile::Flat)
                    } else {
                        None
                    };
                    profile.map_or(Ok(false), |p| {
                        tracing::debug!("Setting {} accel profile to {:?}", device.name(), p);
                        device
                            .config_accel_set_profile(p)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    })
                }

                PROP_LIBINPUT_ACCEL_PROFILES_AVAILABLE => Ok(false),

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
                    method.map_or(Ok(false), |m| {
                        tracing::debug!("Setting {} click method to {:?}", device.name(), m);
                        device
                            .config_click_set_method(m)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    })
                }

                PROP_LIBINPUT_CLICK_METHODS_AVAILABLE => Ok(false),

                PROP_LIBINPUT_DISABLE_WHILE_TYPING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} disable-while-typing to {}", device.name(), enabled != 0);
                    device
                        .config_dwt_set_enabled(enabled != 0)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_HIGH_RESOLUTION_WHEEL_SCROLL_ENABLED => {
                    // I thought there was a way to set this in libinput, but I can't find it.
                    // From what I can tell, for Wayland you need to edit some file under
                    // /etc/libinput, which is pretty lame.
                    Ok(false)
                }

                PROP_LIBINPUT_LEFT_HANDED_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} left-handed to {}", device.name(), enabled != 0);
                    device
                        .config_left_handed_set(enabled != 0)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_NATURAL_SCROLLING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} natural scroll to {}", device.name(), enabled != 0);
                    device
                        .config_scroll_set_natural_scroll_enabled(enabled != 0)
                        .map(|_| true)
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
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_LIBINPUT_SCROLL_METHODS_AVAILABLE => Ok(false),

                PROP_LIBINPUT_TAPPING_ENABLED => {
                    let enabled = value
                        .get::<i32>()
                        .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                    tracing::debug!("Setting {} tapping to {}", device.name(), enabled != 0);
                    device
                        .config_tap_set_enabled(enabled != 0)
                        .map(|_| true)
                        .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                }

                PROP_SYNAPTICS_TAP_ACTION => {
                    if channel.has_property(PROP_LIBINPUT_TAPPING_ENABLED) {
                        // Prefer the libinput setting
                        Ok(false)
                    } else {
                        let tap_action = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = tap_action.iter().any(|&v| v != 0);
                        tracing::debug!("Setting {} tapping to {}", device.name(), enabled);
                        device
                            .config_tap_set_enabled(enabled)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_EDGE_SCROLLING => {
                    if channel.has_property(PROP_LIBINPUT_SCROLL_METHOD_ENABLED) {
                        // Prefer the libinput setting
                        Ok(false)
                    } else {
                        let edge_scrolling = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = edge_scrolling.iter().any(|&v| v != 0);
                        let method = if enabled { ScrollMethod::Edge } else { ScrollMethod::NoScroll };
                        tracing::debug!("Setting {} scroll method to {:?}", device.name(), method);
                        device
                            .config_scroll_set_method(method)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_TWO_FINGER_SCROLLING => {
                    if channel.has_property(PROP_LIBINPUT_SCROLL_METHOD_ENABLED) {
                        // Prefer the libinput setting
                        Ok(false)
                    } else {
                        let two_finger = value
                            .get::<Array<i32>>()
                            .with_context(|| format!("Failed to convert value for pointer property '{property_name}'"))?;
                        let enabled = two_finger.iter().any(|&v| v != 0);
                        let method = if enabled { ScrollMethod::TwoFinger } else { ScrollMethod::NoScroll };
                        tracing::debug!("Setting {} scroll method to {:?}", device.name(), method);
                        device
                            .config_scroll_set_method(method)
                            .map(|_| true)
                            .map_err(|err| anyhow!("Failed to configure pointer device for property '{property_name}': {err:?}"))
                    }
                }

                PROP_SYNAPTICS_CIRCULAR_SCROLLING | PROP_SYNAPTICS_CIRCULAR_SCROLLING_TRIGGER => {
                    // Libinput does not implement the Synaptics circular scrolling feature.
                    Ok(false)
                }

                PROP_TABLET_MODE => {
                    // Overriding absolute/relative mode is not supported with libinput.
                    Ok(false)
                }

                name => Err(anyhow!("Unhandled pointer settings property {name} for device {}", device.name())),
            }
        }

        handle(&self.channel, &mut self.device, property_name, value)
            .inspect_err(|err| tracing::info!("{err}"))
            .unwrap_or(false)
    }
}

impl Drop for PointerConfig {
    fn drop(&mut self) {
        if self.source_token.is_some() {
            tracing::error!("BUG: Xfconf source leak for pointer '{}'", self.device.name());
        }
    }
}

/// Rounds a rotation in degrees to the nearest quarter turn, normalized to [0, 360).
fn round_rotation_degrees(degrees: i32) -> i32 {
    let degrees = degrees.rem_euclid(360);
    (degrees + 45) / 90 * 90 % 360
}

/// Converts the wacom driver's rotation enum, which the settings dialog stores as-is, to the
/// clockwise angle in degrees that it stands for.
fn wacom_rotation_to_degrees(rotation: i32) -> i32 {
    match rotation {
        1 => 90,
        2 => 270,
        3 => 180,
        _ => 0,
    }
}

/// Builds the libinput calibration matrix (the top two rows of a 3x3 affine transform, in
/// row-major order) for a clockwise `rotation` in degrees, rounded to the nearest quarter turn,
/// and a `reflection` of "X", "Y" or "XY".
///
/// The output's own rotation and reflection are deliberately not part of this: absolute input
/// coordinates are mapped through the output's transform when they reach the seat.
fn calibration_matrix(rotation: i32, reflection: &str) -> [f32; 6] {
    const IDENTITY: [f32; 6] = [1., 0., 0., 0., 1., 0.];

    // Rotating or reflecting around the origin moves the coordinates out of libinput's [0,1]
    // space, so each transform also translates them back.
    let rotation = match round_rotation_degrees(rotation) {
        90 => [0., -1., 1., 1., 0., 0.],
        180 => [-1., 0., 1., 0., -1., 1.],
        270 => [0., 1., 0., -1., 0., 1.],
        _ => IDENTITY,
    };
    let reflection = match reflection {
        "X" => [-1., 0., 1., 0., 1., 0.],
        "Y" => [1., 0., 0., 0., -1., 1.],
        "XY" => [-1., 0., 1., 0., -1., 1.],
        _ => IDENTITY,
    };

    [
        rotation[0] * reflection[0] + rotation[1] * reflection[3],
        rotation[0] * reflection[1] + rotation[1] * reflection[4],
        rotation[0] * reflection[2] + rotation[1] * reflection[5] + rotation[2],
        rotation[3] * reflection[0] + rotation[4] * reflection[3],
        rotation[3] * reflection[1] + rotation[4] * reflection[4],
        rotation[3] * reflection[2] + rotation[4] * reflection[5] + rotation[5],
    ]
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

#[cfg(test)]
mod test {
    use super::{calibration_matrix, round_rotation_degrees, wacom_rotation_to_degrees};

    const TOP_LEFT: (f32, f32) = (0., 0.);
    const TOP_RIGHT: (f32, f32) = (1., 0.);
    const BOTTOM_RIGHT: (f32, f32) = (1., 1.);
    const BOTTOM_LEFT: (f32, f32) = (0., 1.);
    const CORNERS: [(f32, f32); 4] = [TOP_LEFT, TOP_RIGHT, BOTTOM_RIGHT, BOTTOM_LEFT];

    fn map(rotation: i32, reflection: &str, corners: [(f32, f32); 4]) -> [(f32, f32); 4] {
        let matrix = calibration_matrix(rotation, reflection);
        corners.map(|(x, y)| (matrix[0] * x + matrix[1] * y + matrix[2], matrix[3] * x + matrix[4] * y + matrix[5]))
    }

    #[test]
    pub fn test_rotation() {
        assert_eq!(map(0, "0", CORNERS), CORNERS);
        assert_eq!(map(90, "0", CORNERS), [TOP_RIGHT, BOTTOM_RIGHT, BOTTOM_LEFT, TOP_LEFT]);
        assert_eq!(map(180, "0", CORNERS), [BOTTOM_RIGHT, BOTTOM_LEFT, TOP_LEFT, TOP_RIGHT]);
        assert_eq!(map(270, "0", CORNERS), [BOTTOM_LEFT, TOP_LEFT, TOP_RIGHT, BOTTOM_RIGHT]);
    }

    #[test]
    pub fn test_rotation_is_rounded_to_quarter_turns() {
        let cases = [
            (0, 0),
            (1, 0),
            (44, 0),
            (45, 90),
            (89, 90),
            (134, 90),
            (135, 180),
            (224, 180),
            (225, 270),
            (269, 270),
            (314, 270),
            (315, 0),
            (359, 0),
        ];

        for (degrees, expected) in cases {
            assert_eq!(round_rotation_degrees(degrees), expected, "rounding {degrees}°");
            assert_eq!(map(degrees, "0", CORNERS), map(expected, "0", CORNERS), "mapping {degrees}°");
        }
    }

    #[test]
    pub fn test_rotation_is_normalized() {
        for (degrees, equivalent) in [(360, 0), (450, 90), (-90, 270), (-360, 0), (1080, 0)] {
            assert_eq!(round_rotation_degrees(degrees), equivalent, "normalizing {degrees}°");
            assert_eq!(map(degrees, "0", CORNERS), map(equivalent, "0", CORNERS), "mapping {degrees}°");
        }
    }

    #[test]
    pub fn test_reflection() {
        assert_eq!(map(0, "X", CORNERS), [TOP_RIGHT, TOP_LEFT, BOTTOM_LEFT, BOTTOM_RIGHT]);
        assert_eq!(map(0, "Y", CORNERS), [BOTTOM_LEFT, BOTTOM_RIGHT, TOP_RIGHT, TOP_LEFT]);
        assert_eq!(map(0, "XY", CORNERS), map(180, "0", CORNERS));
        assert_eq!(map(0, "unrecognized", CORNERS), CORNERS);
    }

    #[test]
    pub fn test_wacom_rotation() {
        assert_eq!(map(wacom_rotation_to_degrees(0), "0", CORNERS), CORNERS);
        assert_eq!(map(wacom_rotation_to_degrees(1), "0", CORNERS), map(90, "0", CORNERS));
        assert_eq!(map(wacom_rotation_to_degrees(2), "0", CORNERS), map(270, "0", CORNERS));
        assert_eq!(map(wacom_rotation_to_degrees(3), "0", CORNERS), map(180, "0", CORNERS));
        assert_eq!(map(wacom_rotation_to_degrees(4), "0", CORNERS), CORNERS);
    }

    #[test]
    pub fn test_reflection_is_applied_before_rotation() {
        assert_eq!(map(90, "X", CORNERS), map(90, "0", map(0, "X", CORNERS)));
        assert_ne!(map(90, "X", CORNERS), map(0, "X", map(90, "0", CORNERS)));
    }
}

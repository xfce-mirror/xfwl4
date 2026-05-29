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

use smithay::{
    reexports::{
        input::{
            AccelProfile, AsRaw, ClickMethod, ClickfingerButtonMap, Device, DeviceCapability, DragLockState as InputDragLockState,
            ScrollButtonLockState as InputScrollButtonLockState, ScrollMethod, SendEventsMode, TapButtonMap,
            ThreeFingerDragState as InputThreeFingerDragState,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
        },
    },
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::protocols::{
    ClientFilter, GlobalData,
    xfce_input_device_list::proto::{
        xfce_input_device_list_private_v1::{Capabilities, XfceInputDeviceListPrivateV1},
        xfce_input_device_v1::{
            AccelProfiles, ButtonMap, ClickMethods, DragLockState, DragState, DwtState, DwtpState, LeftHandedState, MiddleEmulationState,
            NaturalScrollState, ScrollButtonLockState, ScrollMethods, SendEventsModes, TapState, ThreeFingerDragState, XfceInputDeviceV1,
        },
    },
};

const PROTO_VERSION: u32 = proto::__interfaces::XFCE_INPUT_DEVICE_LIST_PRIVATE_V1_INTERFACE.version;

pub struct InputDeviceListState {
    dh: DisplayHandle,
    _global: GlobalId,
    list_instances: Vec<XfceInputDeviceListPrivateV1>,
    input_devices: Vec<InputDevice>,
}

pub trait InputDeviceListHandler: 'static {
    fn input_device_list_state(&mut self) -> &mut InputDeviceListState;
}

struct InputDevice {
    device: Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    instances: Vec<XfceInputDeviceV1>,
}

type AreaRect = (f64, f64, f64, f64);

#[derive(PartialEq)]
struct DeviceSettings {
    send_events: (SendEventsModes, SendEventsModes, SendEventsModes),
    accel_speed: Option<(f64, f64)>,
    accel_profile: Option<(AccelProfiles, AccelProfiles, AccelProfiles)>,
    natural_scroll: Option<(NaturalScrollState, NaturalScrollState)>,
    scroll_method: Option<(ScrollMethods, ScrollMethods, ScrollMethods)>,
    scroll_button: Option<(u32, u32)>,
    scroll_button_lock: Option<(ScrollButtonLockState, ScrollButtonLockState)>,
    click_method: Option<(ClickMethods, ClickMethods, ClickMethods)>,
    clickfinger_button_map: Option<(ButtonMap, ButtonMap)>,
    left_handed: Option<(LeftHandedState, LeftHandedState)>,
    middle_emulation: Option<(MiddleEmulationState, MiddleEmulationState)>,
    tap: Option<(TapState, TapState)>,
    tap_finger_count: Option<u32>,
    tap_button_map: Option<(ButtonMap, ButtonMap)>,
    tap_drag: Option<(DragState, DragState)>,
    tap_drag_lock: Option<(DragLockState, DragLockState)>,
    three_finger_drag: Option<(u32, ThreeFingerDragState, ThreeFingerDragState)>,
    dwt: Option<(DwtState, DwtState)>,
    // TODO: dwt_timeout is not exposed by input v0.10 (added in libinput 1.31)
    dwtp: Option<(DwtpState, DwtpState)>,
    // TODO: dwtp_timeout is not exposed by input v0.10 (added in libinput 1.31)
    rotation: Option<(u32, u32)>,
    calibration: Option<([f64; 6], [f64; 6])>,
    area: Option<(AreaRect, AreaRect)>,
}

impl InputDeviceListState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: InputDeviceListHandler + GlobalDispatch<XfceInputDeviceListPrivateV1, ClientFilter>,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, XfceInputDeviceListPrivateV1, _>(PROTO_VERSION, Box::new(filter));
        Self {
            dh: dh.clone(),
            _global: global,
            list_instances: Default::default(),
            input_devices: Default::default(),
        }
    }

    pub fn input_device_added<D: Dispatch<XfceInputDeviceV1, GlobalData> + 'static>(&mut self, device: Device) {
        let mut device = InputDevice {
            capabilities: capabilities_for_device(&device),
            settings: DeviceSettings::from_device(&device),
            device,
            instances: Default::default(),
        };

        for list_instance in &self.list_instances {
            if let Some(client) = list_instance.client()
                && let Err(err) = send_input_device::<D>(&self.dh, &client, &mut device, list_instance)
            {
                tracing::warn!("Failed to send input device to client: {err}");
            }
        }

        self.input_devices.push(device);
    }

    pub fn input_device_changed(&mut self, device: &Device) {
        if let Some(dev) = self.input_devices.iter_mut().find(|d| d.device == *device) {
            let new = DeviceSettings::from_device(&dev.device);
            for instance in &dev.instances {
                new.emit(Some(&dev.settings), instance);
            }
            dev.settings = new;
        }
    }

    pub fn input_device_removed(&mut self, device: &Device) {
        if let Some(pos) = self.input_devices.iter().position(|d| d.device == *device) {
            let device = self.input_devices.remove(pos);
            for instance in device.instances {
                instance.removed();
            }
        }
    }
}

impl DeviceSettings {
    fn from_device(dev: &Device) -> Self {
        let accel_speed = dev
            .config_accel_is_available()
            .then(|| (dev.config_accel_default_speed(), dev.config_accel_speed()));
        let profiles_vec = dev.config_accel_profiles();
        let accel_profile = (!profiles_vec.is_empty()).then(|| {
            (
                profiles_vec.as_slice().into(),
                dev.config_accel_default_profile().into(),
                dev.config_accel_profile().into(),
            )
        });

        let natural_scroll = dev.config_scroll_has_natural_scroll().then(|| {
            (
                dev.config_scroll_default_natural_scroll_enabled().into(),
                dev.config_scroll_natural_scroll_enabled().into(),
            )
        });

        let scroll_methods_vec = dev.config_scroll_methods();
        let has_button_down = scroll_methods_vec.contains(&ScrollMethod::OnButtonDown);
        let scroll_method = (!scroll_methods_vec.is_empty()).then(|| {
            (
                scroll_methods_vec.as_slice().into(),
                dev.config_scroll_default_method().into(),
                dev.config_scroll_method().into(),
            )
        });
        let scroll_button = has_button_down.then(|| (dev.config_scroll_default_button(), dev.config_scroll_button()));
        let scroll_button_lock = has_button_down.then(|| {
            (
                dev.config_scroll_default_button_lock().into(),
                dev.config_scroll_button_lock().into(),
            )
        });

        let click_methods_vec = dev.config_click_methods();
        let has_clickfinger = click_methods_vec.contains(&ClickMethod::Clickfinger);
        let click_method = (!click_methods_vec.is_empty()).then(|| {
            (
                click_methods_vec.as_slice().into(),
                dev.config_click_default_method().into(),
                dev.config_click_method().into(),
            )
        });
        let clickfinger_button_map = has_clickfinger.then(|| {
            (
                dev.config_click_clickfinger_default_button_map().into(),
                dev.config_click_clickfinger_button_map().into(),
            )
        });

        let left_handed = dev
            .config_left_handed_is_available()
            .then(|| (dev.config_left_handed_default().into(), dev.config_left_handed().into()));
        let middle_emulation = dev.config_middle_emulation_is_available().then(|| {
            (
                dev.config_middle_emulation_default_enabled().into(),
                dev.config_middle_emulation_enabled().into(),
            )
        });

        let tap_fc = dev.config_tap_finger_count();
        let tap_available = tap_fc > 0;
        let tap = tap_available.then(|| (dev.config_tap_default_enabled().into(), dev.config_tap_enabled().into()));
        let tap_finger_count = tap_available.then_some(tap_fc);
        let tap_button_map = tap_available.then(|| (dev.config_tap_default_button_map().into(), dev.config_tap_button_map().into()));
        let tap_drag = tap_available.then(|| (dev.config_tap_default_drag_enabled().into(), dev.config_tap_drag_enabled().into()));
        let tap_drag_lock = tap_available.then(|| (dev.config_tap_default_drag_lock_enabled().into(), drag_lock_state(dev)));

        let max_3fg = dev.config_3fg_drag_get_finger_count();
        let three_finger_drag = (max_3fg > 0).then(|| {
            (
                max_3fg,
                dev.config_3fg_drag_get_default_enabled().into(),
                dev.config_3fg_drag_get_enabled().into(),
            )
        });

        let dwt = dev
            .config_dwt_is_available()
            .then(|| (dev.config_dwt_default_enabled().into(), dev.config_dwt_enabled().into()));
        let dwtp = dev
            .config_dwtp_is_available()
            .then(|| (dev.config_dwtp_default_enabled().into(), dev.config_dwtp_enabled().into()));

        let rotation = dev
            .config_rotation_is_available()
            .then(|| (dev.config_rotation_default_angle(), dev.config_rotation_angle()));

        let calibration = dev.config_calibration_has_matrix().then(|| {
            const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
            let to_f64 = |a: [f32; 6]| [a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64, a[4] as f64, a[5] as f64];
            (
                to_f64(dev.config_calibration_default_matrix().unwrap_or(IDENTITY)),
                to_f64(dev.config_calibration_matrix().unwrap_or(IDENTITY)),
            )
        });

        let area = dev.config_area_has_rectangle().then(|| {
            let d = dev.config_area_get_default_rectangle();
            let c = dev.config_area_get_rectangle();
            ((d.x1, d.y1, d.x2, d.y2), (c.x1, c.y1, c.x2, c.y2))
        });

        Self {
            send_events: (
                dev.config_send_events_modes().into(),
                send_events_default_mode(dev).into(),
                dev.config_send_events_mode().into(),
            ),
            accel_speed,
            accel_profile,
            natural_scroll,
            scroll_method,
            scroll_button,
            scroll_button_lock,
            click_method,
            clickfinger_button_map,
            left_handed,
            middle_emulation,
            tap,
            tap_finger_count,
            tap_button_map,
            tap_drag,
            tap_drag_lock,
            three_finger_drag,
            dwt,
            dwtp,
            rotation,
            calibration,
            area,
        }
    }

    fn emit(&self, prev: Option<&Self>, instance: &XfceInputDeviceV1) {
        macro_rules! changed {
            ($f:ident) => {
                prev.map_or(true, |p| p.$f != self.$f)
            };
        }

        if changed!(send_events) {
            let (supported, default, current) = self.send_events;
            instance.send_events(supported, default, current);
        }
        if changed!(accel_speed)
            && let Some((default, current)) = self.accel_speed
        {
            instance.accel_speed(default, current);
        }
        if changed!(accel_profile)
            && let Some((supported, default, current)) = self.accel_profile
        {
            instance.accel_profile(supported, default, current);
        }
        if changed!(natural_scroll)
            && let Some((default, current)) = self.natural_scroll
        {
            instance.natural_scroll(default, current);
        }
        if changed!(scroll_method)
            && let Some((supported, default, current)) = self.scroll_method
        {
            instance.scroll_method(supported, default, current);
        }
        if changed!(scroll_button)
            && let Some((default, current)) = self.scroll_button
        {
            instance.scroll_button(default, current);
        }
        if changed!(scroll_button_lock)
            && let Some((default, current)) = self.scroll_button_lock
        {
            instance.scroll_button_lock(default, current);
        }
        if changed!(click_method)
            && let Some((supported, default, current)) = self.click_method
        {
            instance.click_method(supported, default, current);
        }
        if changed!(clickfinger_button_map)
            && let Some((default, current)) = self.clickfinger_button_map
        {
            instance.clickfinger_button_map(default, current);
        }
        if changed!(left_handed)
            && let Some((default, current)) = self.left_handed
        {
            instance.left_handed(default, current);
        }
        if changed!(middle_emulation)
            && let Some((default, current)) = self.middle_emulation
        {
            instance.middle_emulation(default, current);
        }
        if changed!(tap)
            && let Some((default, current)) = self.tap
        {
            instance.tap(default, current);
        }
        if changed!(tap_finger_count)
            && let Some(count) = self.tap_finger_count
        {
            instance.tap_finger_count(count);
        }
        if changed!(tap_button_map)
            && let Some((default, current)) = self.tap_button_map
        {
            instance.tap_button_map(default, current);
        }
        if changed!(tap_drag)
            && let Some((default, current)) = self.tap_drag
        {
            instance.tap_drag(default, current);
        }
        if changed!(tap_drag_lock)
            && let Some((default, current)) = self.tap_drag_lock
        {
            instance.tap_drag_lock(default, current);
        }
        if changed!(three_finger_drag)
            && let Some((max_fingers, default, current)) = self.three_finger_drag
        {
            instance.three_finger_drag(max_fingers, default, current);
        }
        if changed!(dwt)
            && let Some((default, current)) = self.dwt
        {
            instance.dwt(default, current);
        }
        if changed!(dwtp)
            && let Some((default, current)) = self.dwtp
        {
            instance.dwtp(default, current);
        }
        if changed!(rotation)
            && let Some((default, current)) = self.rotation
        {
            instance.rotation(default, current);
        }
        if changed!(calibration)
            && let Some((default, current)) = self.calibration
        {
            instance.calibration(
                default[0], default[1], default[2], default[3], default[4], default[5], current[0], current[1], current[2], current[3],
                current[4], current[5],
            );
        }
        if changed!(area)
            && let Some((default, current)) = self.area
        {
            instance.area(
                default.0, default.1, default.2, default.3, current.0, current.1, current.2, current.3,
            );
        }
    }
}

impl<D> GlobalDispatch2<XfceInputDeviceListPrivateV1, D> for ClientFilter
where
    D: InputDeviceListHandler + Dispatch<XfceInputDeviceListPrivateV1, GlobalData> + Dispatch<XfceInputDeviceV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<XfceInputDeviceListPrivateV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        let state = state.input_device_list_state();
        for device in &mut state.input_devices {
            if let Err(err) = send_input_device::<D>(handle, client, device, &instance) {
                tracing::warn!("Failed to send input device to client: {err}");
            }
        }
        state.list_instances.push(instance);
    }

    fn can_view(&self, client: &Client) -> bool {
        self(client)
    }
}

impl<D: InputDeviceListHandler> Dispatch2<XfceInputDeviceListPrivateV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceInputDeviceListPrivateV1,
        request: <XfceInputDeviceListPrivateV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_input_device_list_private_v1::Request;

        match request {
            Request::Destroy => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceInputDeviceListPrivateV1) {
        state
            .input_device_list_state()
            .list_instances
            .retain(|instance| instance != resource);
    }
}

impl<D: InputDeviceListHandler> Dispatch2<XfceInputDeviceV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceInputDeviceV1,
        request: <XfceInputDeviceV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_input_device_v1::Request;

        match request {
            Request::Release => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceInputDeviceV1) {
        for device in &mut state.input_device_list_state().input_devices {
            device.instances.retain(|instance| instance != resource);
        }
    }
}

// Works for both the supported bitmask and a single default/current mode:
// SendEventsMode::ENABLED and SendEventsModes::Enabled are both 0, so OR-ing in
// the protocol's Enabled bit is a no-op. A single DISABLED or
// DISABLED_ON_EXTERNAL_MOUSE value maps to exactly that bit; the supported
// bitmask round-trips libinput's returned bits.
impl From<SendEventsMode> for SendEventsModes {
    fn from(value: SendEventsMode) -> Self {
        [
            (SendEventsMode::ENABLED, SendEventsModes::Enabled),
            (SendEventsMode::DISABLED, SendEventsModes::Disabled),
            (SendEventsMode::DISABLED_ON_EXTERNAL_MOUSE, SendEventsModes::DisabledOnExternalMouse),
        ]
        .into_iter()
        .fold(SendEventsModes::empty(), |modes, (dev_mode, mode)| {
            if value.contains(dev_mode) { modes | mode } else { modes }
        })
    }
}

impl From<AccelProfile> for AccelProfiles {
    fn from(value: AccelProfile) -> Self {
        match value {
            AccelProfile::Flat => Self::Flat,
            AccelProfile::Adaptive => Self::Adaptive,
            AccelProfile::Custom => Self::Custom,
            _ => {
                tracing::warn!(?value, "unrecognized libinput AccelProfile; reporting as None");
                Self::None
            }
        }
    }
}

impl From<&[AccelProfile]> for AccelProfiles {
    fn from(value: &[AccelProfile]) -> Self {
        value.iter().copied().map(Self::from).fold(Self::empty(), |a, b| a | b)
    }
}

impl From<Option<AccelProfile>> for AccelProfiles {
    fn from(value: Option<AccelProfile>) -> Self {
        value.map(Self::from).unwrap_or(Self::None)
    }
}

impl From<ScrollMethod> for ScrollMethods {
    fn from(value: ScrollMethod) -> Self {
        match value {
            ScrollMethod::NoScroll => Self::NoScroll,
            ScrollMethod::TwoFinger => Self::TwoFinger,
            ScrollMethod::Edge => Self::Edge,
            ScrollMethod::OnButtonDown => Self::OnButtonDown,
            _ => {
                tracing::warn!(?value, "unrecognized libinput ScrollMethod; reporting as NoScroll");
                Self::NoScroll
            }
        }
    }
}

impl From<&[ScrollMethod]> for ScrollMethods {
    fn from(value: &[ScrollMethod]) -> Self {
        value.iter().copied().map(Self::from).fold(Self::empty(), |a, b| a | b)
    }
}

impl From<Option<ScrollMethod>> for ScrollMethods {
    fn from(value: Option<ScrollMethod>) -> Self {
        value.map(Self::from).unwrap_or(Self::NoScroll)
    }
}

impl From<ClickMethod> for ClickMethods {
    fn from(value: ClickMethod) -> Self {
        match value {
            ClickMethod::ButtonAreas => Self::ButtonAreas,
            ClickMethod::Clickfinger => Self::Clickfinger,
            _ => {
                tracing::warn!(?value, "unrecognized libinput ClickMethod; reporting as None");
                Self::None
            }
        }
    }
}

impl From<&[ClickMethod]> for ClickMethods {
    fn from(value: &[ClickMethod]) -> Self {
        value.iter().copied().map(Self::from).fold(Self::empty(), |a, b| a | b)
    }
}

impl From<Option<ClickMethod>> for ClickMethods {
    fn from(value: Option<ClickMethod>) -> Self {
        value.map(Self::from).unwrap_or(Self::None)
    }
}

impl From<InputScrollButtonLockState> for ScrollButtonLockState {
    fn from(value: InputScrollButtonLockState) -> Self {
        match value {
            InputScrollButtonLockState::Disabled => Self::Disabled,
            InputScrollButtonLockState::Enabled => Self::Enabled,
        }
    }
}

impl From<InputDragLockState> for DragLockState {
    fn from(value: InputDragLockState) -> Self {
        match value {
            InputDragLockState::Disabled => Self::Disabled,
            InputDragLockState::EnabledTimeout => Self::EnabledTimeout,
            InputDragLockState::EnabledSticky => Self::EnabledSticky,
            _ => {
                tracing::warn!(?value, "unrecognized libinput DragLockState; reporting as Disabled");
                Self::Disabled
            }
        }
    }
}

impl From<ClickfingerButtonMap> for ButtonMap {
    fn from(value: ClickfingerButtonMap) -> Self {
        match value {
            ClickfingerButtonMap::LeftRightMiddle => Self::Lrm,
            ClickfingerButtonMap::LeftMiddleRight => Self::Lmr,
            _ => {
                tracing::warn!(?value, "unrecognized libinput ClickfingerButtonMap; reporting as Lrm");
                Self::Lrm
            }
        }
    }
}

impl From<InputThreeFingerDragState> for ThreeFingerDragState {
    fn from(value: InputThreeFingerDragState) -> Self {
        match value {
            InputThreeFingerDragState::Disabled => Self::Disabled,
            InputThreeFingerDragState::EnabledThreeFinger => Self::Enabled3fg,
            InputThreeFingerDragState::EnabledFourFinger => Self::Enabled4fg,
            _ => {
                tracing::warn!(?value, "unrecognized libinput ThreeFingerDragState; reporting as Disabled");
                Self::Disabled
            }
        }
    }
}

impl From<TapButtonMap> for ButtonMap {
    fn from(value: TapButtonMap) -> Self {
        match value {
            TapButtonMap::LeftRightMiddle => Self::Lrm,
            TapButtonMap::LeftMiddleRight => Self::Lmr,
            _ => {
                tracing::warn!(?value, "unrecognized libinput TapButtonMap; reporting as Lrm");
                Self::Lrm
            }
        }
    }
}

impl From<Option<TapButtonMap>> for ButtonMap {
    fn from(value: Option<TapButtonMap>) -> Self {
        value.map(Self::from).unwrap_or(Self::Lrm)
    }
}

macro_rules! impl_bool_state {
    ($t:ty) => {
        impl From<bool> for $t {
            fn from(value: bool) -> Self {
                if value { Self::Enabled } else { Self::Disabled }
            }
        }
    };
}

impl_bool_state!(NaturalScrollState);
impl_bool_state!(LeftHandedState);
impl_bool_state!(MiddleEmulationState);
impl_bool_state!(TapState);
impl_bool_state!(DragState);
impl_bool_state!(DwtState);
impl_bool_state!(DwtpState);

fn send_input_device<D: Dispatch<XfceInputDeviceV1, GlobalData> + 'static>(
    dh: &DisplayHandle,
    client: &Client,
    device: &mut InputDevice,
    list_instance: &XfceInputDeviceListPrivateV1,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<XfceInputDeviceV1, _, D>(dh, list_instance.version(), GlobalData)?;
    list_instance.device(&instance, device.device.name().into_owned(), device.capabilities);
    device.settings.emit(None, &instance);
    device.instances.push(instance);
    Ok(())
}

fn capabilities_for_device(device: &Device) -> Capabilities {
    [
        (DeviceCapability::Keyboard, Capabilities::Keyboard),
        (DeviceCapability::Pointer, Capabilities::Pointer),
        (DeviceCapability::Touch, Capabilities::Touch),
        (DeviceCapability::TabletTool, Capabilities::TabletTool),
        (DeviceCapability::TabletPad, Capabilities::TabletPad),
        (DeviceCapability::Gesture, Capabilities::Gesture),
        (DeviceCapability::Switch, Capabilities::Switch),
    ]
    .into_iter()
    .fold(Capabilities::empty(), |caps, (dev_cap, cap)| {
        if device.has_capability(dev_cap) { caps | cap } else { caps }
    })
}

// Not exposed by the bindings.
fn send_events_default_mode(device: &Device) -> SendEventsMode {
    // SAFETY: the input crate's Device owns a valid libinput_device pointer
    // for its lifetime; we only read.
    SendEventsMode::from_bits_truncate(unsafe {
        smithay::reexports::input::ffi::libinput_device_config_send_events_get_default_mode(device.as_raw() as *mut _)
    })
}

// Bypasses input 0.10's `config_tap_drag_lock_enabled` wrapper, which returns bool and panics on
// EnabledSticky.
fn drag_lock_state(device: &Device) -> DragLockState {
    use smithay::reexports::input::ffi;
    // SAFETY: the input crate's Device owns a valid libinput_device pointer for its lifetime; we
    // only read.
    match unsafe { ffi::libinput_device_config_tap_get_drag_lock_enabled(device.as_raw() as *mut _) } {
        ffi::libinput_config_drag_lock_state_LIBINPUT_CONFIG_DRAG_LOCK_DISABLED => DragLockState::Disabled,
        ffi::libinput_config_drag_lock_state_LIBINPUT_CONFIG_DRAG_LOCK_ENABLED_TIMEOUT => DragLockState::EnabledTimeout,
        ffi::libinput_config_drag_lock_state_LIBINPUT_CONFIG_DRAG_LOCK_ENABLED_STICKY => DragLockState::EnabledSticky,
        _ => DragLockState::Disabled,
    }
}

pub mod proto {
    use smithay::reexports::wayland_server;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::backend as wayland_backend;

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-input-device-list-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-input-device-list-private-v1.xml");
}

use smithay::reexports::calloop::LoopHandle;
use xfconf::ChannelExtManual;

use crate::{Xfwl4State, backend::Backend, config::XFWM4_CHANNEL_NAME, util::CalloopXfconfSource};

const PROP_CYCLE_WORKSPACES: &str = "/general/cycle_workspaces";
const PROP_SCROLL_WORKSPACES: &str = "/general/scroll_workspaces";
const PROP_TOGGLE_WORKSPACES: &str = "/general/toggle_workspaces";
const PROP_WRAP_WORKSPACES: &str = "/general/wrap_workspaces";

pub struct Xfwl4Config {
    channel: xfconf::Channel,

    pub cycle_workspaces: bool,
    pub scroll_workspaces: bool,
    pub toggle_workspaces: bool,
    pub wrap_workspaces: bool,
}

impl Xfwl4Config {
    pub fn new<BackendData: Backend + 'static>(handle: LoopHandle<'static, Xfwl4State<BackendData>>) -> Self {
        let mut config = Self {
            channel: xfconf::Channel::new(XFWM4_CHANNEL_NAME),

            cycle_workspaces: false,
            scroll_workspaces: true,
            toggle_workspaces: false,
            wrap_workspaces: false,
        };

        let source = CalloopXfconfSource::new(config.channel.clone(), []);
        handle
            .insert_source(source, |(property_name, value), _, state| {
                state.config.handle_property_changed(&property_name, value)
            })
            .expect("Failed to register xfconf xfwm4 source with event loop");

        for property_name in [
            PROP_CYCLE_WORKSPACES,
            PROP_SCROLL_WORKSPACES,
            PROP_TOGGLE_WORKSPACES,
            PROP_WRAP_WORKSPACES,
        ] {
            if let Some(value) = config.channel.get_property_value(property_name) {
                config.handle_property_changed(property_name, value);
            }
        }

        config
    }

    fn handle_property_changed(&mut self, property_name: &str, value: glib::Value) {
        match property_name {
            PROP_CYCLE_WORKSPACES => {
                if let Ok(v) = value.get::<bool>() {
                    self.cycle_workspaces = v;
                }
            }

            PROP_SCROLL_WORKSPACES => {
                if let Ok(v) = value.get::<bool>() {
                    self.scroll_workspaces = v;
                }
            }

            PROP_TOGGLE_WORKSPACES => {
                if let Ok(v) = value.get::<bool>() {
                    self.toggle_workspaces = v;
                }
            }

            PROP_WRAP_WORKSPACES => {
                if let Ok(v) = value.get::<bool>() {
                    self.wrap_workspaces = v;
                }
            }

            _ => (),
        }
    }
}

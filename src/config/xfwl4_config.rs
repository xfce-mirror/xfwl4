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

mod imp {
    use glib::{ParamSpecBuilderExt, subclass::prelude::*};
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct Xfwl4Config {
        channel: RefCell<Option<xfconf::Channel>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Xfwl4Config {
        const NAME: &'static str = "Xfwl4Config";
        type Type = super::Xfwl4Config;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for Xfwl4Config {
        fn properties() -> &'static [glib::ParamSpec] {
            use std::sync::OnceLock;
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();

            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<xfconf::Channel>("channel")
                        .readwrite()
                        .construct_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "channel" => {
                    let channel = value.get().expect("wrong type for xfconf::Channel");
                    self.channel.replace(channel);
                }
                name => tracing::error!("Invalid property name {name}"),
            }
        }
    }
}

glib::wrapper! {
    pub struct Xfwl4Config(ObjectSubclass<imp::Xfwl4Config>);
}

impl Xfwl4Config {
    pub fn new(channel: xfconf::Channel) -> Self {
        glib::Object::builder().property("channel", channel).build()
    }
}

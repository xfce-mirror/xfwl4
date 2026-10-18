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

use glib::{prelude::*, translate::*};
use gtk::{Actionable, Bin, Buildable, Container, MenuItem, Widget, ffi};

glib::wrapper! {
    pub struct ImageMenuItem(Object<ffi::GtkImageMenuItem, ffi::GtkImageMenuItemClass>)
        @extends Bin, Container, Widget, MenuItem, @implements Buildable, Actionable;

    match fn {
        type_ => || ffi::gtk_image_menu_item_get_type(),
    }
}

impl ImageMenuItem {
    pub fn builder() -> ImageMenuItemBuilder {
        ImageMenuItemBuilder::new()
    }
}

pub struct ImageMenuItemBuilder {
    builder: glib::object::ObjectBuilder<'static, ImageMenuItem>,
}

impl ImageMenuItemBuilder {
    fn new() -> Self {
        Self {
            builder: glib::object::Object::builder(),
        }
    }

    pub fn label(self, label: impl Into<glib::GString>) -> Self {
        Self {
            builder: self.builder.property("label", label.into()),
        }
    }

    pub fn image(self, image: &impl IsA<Widget>) -> Self {
        Self {
            builder: self.builder.property("image", image),
        }
    }

    pub fn use_underline(self, use_underline: bool) -> Self {
        Self {
            builder: self.builder.property("use-underline", use_underline),
        }
    }

    pub fn sensitive(self, sensitive: bool) -> Self {
        Self {
            builder: self.builder.property("sensitive", sensitive),
        }
    }

    pub fn build(self) -> ImageMenuItem {
        self.builder.build()
    }
}

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

use serde::de::{Error as _, MapAccess};
use smithay::utils::Rectangle;

pub mod serde_rectangle {
    use std::{fmt, marker::PhantomData};

    use serde::{
        Deserializer, Serializer,
        de::{MapAccess, Visitor},
        ser::SerializeMap,
    };
    use smithay::utils::Rectangle;

    use super::rectangle_visit_map;

    pub fn serialize<S, Kind>(rect: &Rectangle<i32, Kind>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("x", &rect.loc.x)?;
        map.serialize_entry("y", &rect.loc.y)?;
        map.serialize_entry("w", &rect.size.w)?;
        map.serialize_entry("h", &rect.size.h)?;
        map.end()
    }

    pub fn deserialize<'de, D, Kind>(deserializer: D) -> Result<Rectangle<i32, Kind>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RectangleVisitor<Kind>(PhantomData<Kind>);

        impl<'de, Kind> Visitor<'de> for RectangleVisitor<Kind> {
            type Value = Rectangle<i32, Kind>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map with x, y, w, and h keys")
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                rectangle_visit_map(map)
            }
        }

        deserializer.deserialize_map(RectangleVisitor(PhantomData::<Kind>))
    }
}

pub mod serde_optional_rectangle {
    use std::{fmt, marker::PhantomData};

    use serde::{
        Deserializer, Serializer,
        de::{self, MapAccess, Visitor},
    };
    use smithay::utils::Rectangle;

    use super::rectangle_visit_map;

    pub fn serialize<S, Kind>(rect: &Option<Rectangle<i32, Kind>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(rect) = rect {
            super::serde_rectangle::serialize(rect, serializer)
        } else {
            serializer.serialize_none()
        }
    }

    pub fn deserialize<'de, D, Kind>(deserializer: D) -> Result<Option<Rectangle<i32, Kind>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionalRectangleVisitor<Kind>(PhantomData<Kind>);

        impl<'de, Kind> Visitor<'de> for OptionalRectangleVisitor<Kind> {
            type Value = Option<Rectangle<i32, Kind>>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map with x, y, w, and h keys")
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_map(self)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                rectangle_visit_map(map).map(Some)
            }
        }

        deserializer.deserialize_map(OptionalRectangleVisitor(PhantomData::<Kind>))
    }
}

fn rectangle_visit_map<'de, A, Kind>(mut map: A) -> Result<Rectangle<i32, Kind>, A::Error>
where
    A: MapAccess<'de>,
{
    let mut x = None;
    let mut y = None;
    let mut w = None;
    let mut h = None;

    while let Some((key, value)) = map.next_entry::<String, i32>()? {
        match key.as_str() {
            "x" => x = Some(value),
            "y" => y = Some(value),
            "w" => w = Some(value),
            "h" => h = Some(value),
            _ => (),
        }
    }

    match (x, y, w, h) {
        (Some(x), Some(y), Some(w), Some(h)) => Ok(Rectangle::<_, Kind>::new((x, y).into(), (w.max(0), h.max(0)).into())),
        _ => Err(A::Error::custom("missing fields in rectangle")),
    }
}

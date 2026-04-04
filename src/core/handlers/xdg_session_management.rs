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

use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::PathBuf,
};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use smithay::utils::{Logical, Rectangle};

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub app_id: Option<String>,
    pub toplevels: HashMap<String, ToplevelSessionData>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToplevelSessionData {
    pub workspace: Option<u32>,
    #[serde(with = "serde_rectangle")]
    pub geometry: Rectangle<i32, Logical>,
    pub minimized: bool,
    pub maximized: bool,
    pub shaded: bool,
    pub fullscreen: bool,
    pub tile_mode: ToplevelTileMode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToplevelGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToplevelTileMode {
    None,
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl SessionData {
    fn session_path(session_id: &str) -> PathBuf {
        let mut path = glib::user_state_dir();
        path.push("xfce4");
        path.push("xfwl4");
        path.push(format!("{session_id}.json"));
        path
    }

    pub fn load(session_id: &str) -> Option<Self> {
        let path = Self::session_path(session_id);
        let f = fs::File::open(&path)
            .inspect_err(|err| {
                if err.kind() != ErrorKind::NotFound {
                    tracing::warn!("Failed to open session data file '{}': {err}", path.display());
                }
            })
            .ok()?;
        serde_json::from_reader::<_, SessionData>(f)
            .inspect_err(|err| tracing::warn!("Failed to deserialize session data file '{}': {err}", path.display()))
            .ok()
    }

    pub fn store(&self) {
        let do_store = || -> anyhow::Result<()> {
            let serialized = serde_json::to_vec(self)?;
            let path = Self::session_path(&self.session_id);
            let parent = path.parent().ok_or_else(|| anyhow!("BUG: can't get parent of session file"))?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serialized)?;
            file.persist(path)?;
            Ok(())
        };

        if let Err(err) = do_store() {
            tracing::warn!("Failed to store data for session '{}': {err}", self.session_id);
        }
    }
}

mod serde_rectangle {
    use serde::{
        Deserializer, Serializer,
        de::{Error as _, Visitor},
        ser::SerializeMap,
    };
    use smithay::utils::{Logical, Rectangle};

    pub fn serialize<S: Serializer>(v: &Rectangle<i32, Logical>, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("x", &v.loc.x)?;
        map.serialize_entry("y", &v.loc.y)?;
        map.serialize_entry("w", &v.size.w)?;
        map.serialize_entry("h", &v.size.h)?;
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Rectangle<i32, Logical>, D::Error> {
        struct RectangleVisitor;

        impl<'de> Visitor<'de> for RectangleVisitor {
            type Value = Rectangle<i32, Logical>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map with x, y, w, and h keys, all 32-bit integers")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
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
                    (Some(x), Some(y), Some(w), Some(h)) => Ok(Rectangle::new((x, y).into(), (w, h).into())),
                    _ => Err(A::Error::custom("missing rectangle fields")),
                }
            }
        }

        deserializer.deserialize_map(RectangleVisitor)
    }
}

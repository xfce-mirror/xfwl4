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
    fmt,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    str::FromStr,
};

use anyhow::{Context, anyhow};

#[derive(Debug, Clone, Copy)]
pub enum RcValueType {
    String,
    Int,
    Bool,
    Color,
}

#[derive(Debug, Clone)]
pub enum RcValue {
    String(String),
    Int(i32),
    Bool(bool),
    Color(RcColor),
}

impl RcValue {
    pub fn parse(s: &str, ty: RcValueType) -> anyhow::Result<Self> {
        match ty {
            RcValueType::String => Ok(RcValue::String(s.to_owned())),
            RcValueType::Int => Ok(RcValue::Int(s.parse()?)),
            RcValueType::Bool => Ok(RcValue::Bool(s.parse()?)),
            RcValueType::Color => Ok(RcValue::Color(s.parse()?)),
        }
    }

    pub fn from_gvalue(v: glib::Value, ty: RcValueType) -> anyhow::Result<Self> {
        match ty {
            RcValueType::String => Ok(RcValue::String(v.get()?)),
            RcValueType::Int => Ok(RcValue::Int(v.get()?)),
            RcValueType::Bool => Ok(RcValue::Bool(v.get()?)),
            RcValueType::Color => {
                if let Ok(s) = v.get::<String>() {
                    Ok(RcValue::Color(RcColor::Named(s)))
                } else if let Ok(arr) = v.get::<xfconf::Array<f64>>() {
                    let mut iter = arr.into_iter();
                    match (iter.next(), iter.next(), iter.next(), iter.next()) {
                        (Some(red), Some(green), Some(blue), alpha) => {
                            fn conv(v: f64) -> u8 {
                                (v * 255.).round().clamp(0., 255.) as u8
                            }

                            Ok(RcValue::Color(RcColor::Rgba {
                                red: conv(red),
                                green: conv(green),
                                blue: conv(blue),
                                alpha: alpha.map(conv).unwrap_or(255),
                            }))
                        }
                        _ => Err(anyhow!("invalid number of array elements for type color")),
                    }
                } else {
                    Err(anyhow!("invalid gvalue type for color"))
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum RcColor {
    Named(String),
    Rgba { red: u8, green: u8, blue: u8, alpha: u8 },
}

impl FromStr for RcColor {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hex) = s.strip_prefix('#') {
            let pairs = (0..hex.len())
                .step_by(2)
                .map(|i| {
                    hex.get(i..(i + 2))
                        .ok_or_else(|| anyhow!("hex color string incomplete"))
                        .and_then(|pair| u8::from_str_radix(pair, 16).map_err(|_| anyhow!("invalid hex characters in color string")))
                })
                .collect::<Result<Vec<u8>, _>>()?;

            match pairs.as_slice() {
                [r, g, b] => Ok(RcColor::Rgba {
                    red: *r,
                    green: *g,
                    blue: *b,
                    alpha: u8::MAX,
                }),
                [r, g, b, a] => Ok(RcColor::Rgba {
                    red: *r,
                    green: *g,
                    blue: *b,
                    alpha: *a,
                }),
                _ => Err(anyhow!("hex color must have 3 or 4 byte pairs")),
            }
        } else if !s.trim().is_empty() {
            Ok(RcColor::Named(s.to_owned()))
        } else {
            Err(anyhow!("value was empty"))
        }
    }
}

#[derive(Clone)]
pub struct RcSetting {
    pub name: &'static str,
    ty: RcValueType,
    in_xfconf: bool,
    is_decoration_setting: bool,
    required: bool,
    value: Option<RcValue>,
}

impl RcSetting {
    pub const fn new(name: &'static str, ty: RcValueType, in_xfconf: bool, is_decoration_setting: bool, required: bool) -> Self {
        Self {
            name,
            ty,
            in_xfconf,
            is_decoration_setting,
            required,
            value: None,
        }
    }

    pub fn set_from_str(&mut self, s: &str) -> anyhow::Result<()> {
        self.value = Some(RcValue::parse(s, self.ty)?);
        Ok(())
    }

    pub fn set_from_xfconf(&mut self, v: glib::Value) -> anyhow::Result<()> {
        if self.in_xfconf {
            self.value = Some(RcValue::from_gvalue(v, self.ty)?);
            Ok(())
        } else {
            Err(anyhow!("Setting {} cannot come from xfconf", self.name))
        }
    }

    pub fn in_xfconf(&self) -> bool {
        self.in_xfconf
    }

    pub fn is_decoration_setting(&self) -> bool {
        self.is_decoration_setting
    }

    pub fn as_string(&self) -> Option<String> {
        match &self.value {
            Some(RcValue::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match &self.value {
            Some(RcValue::Int(i)) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match &self.value {
            Some(RcValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<RcColor> {
        match &self.value {
            Some(RcValue::Color(c)) => Some(c.clone()),
            _ => None,
        }
    }
}

impl fmt::Debug for RcSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RcSetting")
            .field("name", &self.name)
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

pub fn parse<P: AsRef<Path>>(path: P, settings: &mut HashMap<String, RcSetting>, allow_value_errors: bool) -> anyhow::Result<()> {
    let path = path.as_ref();
    let fname = path.to_string_lossy();

    let f = File::open(path)?;
    let reader = BufReader::new(f);

    for line in reader.lines() {
        let line = line.with_context(|| format!("Failed to read RC file {fname}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.splitn(2, "=");
        match (parts.next(), parts.next()) {
            (Some(key), Some(value)) => {
                if let Some(setting) = settings.get_mut(key) {
                    if let Err(err) = setting.set_from_str(value) {
                        if allow_value_errors {
                            tracing::warn!("Invalid value for setting {key}: {err}");
                        } else {
                            Err(anyhow!("Invalid value for setting {key}: {err}"))?;
                        }
                    }
                } else {
                    tracing::info!("Unknown setting '{key}'");
                }
            }
            _ => tracing::warn!("Invalid settings line '{line}'"),
        }
    }

    settings
        .iter()
        .map(|(name, setting)| {
            if setting.required && setting.value.is_none() {
                Err(anyhow!("Missing value for required setting {name}"))
            } else {
                Ok(())
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ())
}

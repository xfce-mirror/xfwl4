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

/// xfwm4 implements a custom XPM parser that allows theme authors to use symbolic GTK theme color
/// names in their XPM files, which are replaced at runtime with the colors in the current GTK
/// theme.  Instead of implementing a full custom parser here, we instead just pre-process the XPM
/// data (which is just text) to find any instances of the theme color names, and replace them with
/// the colors in the actual theme.  Then the image loading code can simply use GdkPixbuf's XPM
/// parser to load the pre-processed data with the correctly-substituted colors.
use std::{collections::HashMap, path::Path};

use anyhow::{Context, anyhow};
use gtk::gdk;

const XPM_COLOR_KEYS: &[&str] = &["c", "s", "m", "g", "g4"];

pub fn load_xpm_with_color_substitution<P: AsRef<Path>>(
    path: P,
    color_symbols: &HashMap<String, gdk::RGBA>,
) -> anyhow::Result<Vec<String>> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path).with_context(|| format!("Failed to read XPM file: {}", path.display()))?;

    let quoted_strings = extract_quoted_strings(&content);

    let &(header_start, header_end) = quoted_strings
        .first()
        .ok_or_else(|| anyhow!("No data found in XPM file: {}", path.display()))?;
    let header = content
        .get(header_start..header_end)
        .ok_or_else(|| anyhow!("Invalid string range in XPM file: {}", path.display()))?;

    let header_parts = header.split_whitespace().collect::<Vec<_>>();
    let num_colors: usize = header_parts
        .get(2)
        .ok_or_else(|| anyhow!("Invalid XPM header in {}", path.display()))?
        .parse()
        .with_context(|| format!("Failed to parse num_colors from XPM header in {}", path.display()))?;
    let cpp: usize = header_parts
        .get(3)
        .ok_or_else(|| anyhow!("Invalid XPM header in {}", path.display()))?
        .parse()
        .with_context(|| format!("Failed to parse chars_per_pixel from XPM header in {}", path.display()))?;

    (quoted_strings.len() > num_colors).then_some(()).ok_or_else(|| {
        anyhow!(
            "XPM file {} declares {} colors but only has {} data strings",
            path.display(),
            num_colors,
            quoted_strings.len().saturating_sub(1),
        )
    })?;

    quoted_strings
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| {
            let s = content
                .get(start..end)
                .ok_or_else(|| anyhow!("Invalid string range in XPM file: {}", path.display()))?;
            if i >= 1 && i <= num_colors {
                Ok(substitute_color_line(s, cpp, color_symbols).unwrap_or_else(|| s.to_owned()))
            } else {
                Ok(s.to_owned())
            }
        })
        .collect()
}

fn extract_quoted_strings(content: &str) -> Vec<(usize, usize)> {
    content
        .bytes()
        .enumerate()
        .scan((None::<usize>, false), |(open, skip_next), (i, b)| {
            if *skip_next {
                *skip_next = false;
                Some(None)
            } else {
                match (*open, b) {
                    (None, b'"') => {
                        *open = Some(i + 1);
                        Some(None)
                    }
                    (Some(start), b'"') => {
                        *open = None;
                        Some(Some((start, i)))
                    }
                    (Some(_), b'\\') => {
                        *skip_next = true;
                        Some(None)
                    }
                    _ => Some(None),
                }
            }
        })
        .flatten()
        .collect()
}

fn substitute_color_line(line: &str, cpp: usize, color_symbols: &HashMap<String, gdk::RGBA>) -> Option<String> {
    let pixel_chars = line.get(..cpp)?;
    let tokens = line.get(cpp..)?.split_whitespace().collect::<Vec<_>>();

    let key_positions = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, t)| XPM_COLOR_KEYS.contains(t).then_some(i))
        .chain(std::iter::once(tokens.len()))
        .collect::<Vec<_>>();

    let entries = key_positions
        .windows(2)
        .filter_map(|w| {
            let &start = w.first()?;
            let &end = w.last()?;
            Some((*tokens.get(start)?, tokens.get(start + 1..end)?))
        })
        .collect::<Vec<_>>();

    let symbolic_name = entries.iter().find(|(k, _)| *k == "s").and_then(|(_, v)| v.first().copied())?;
    let rgba = color_symbols.get(symbolic_name)?;

    let hex_color = format!(
        "#{:02X}{:02X}{:02X}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    );

    let new_entries = entries
        .iter()
        .map(|(k, v)| {
            if *k == "c" {
                format!("{k} {hex_color}")
            } else {
                format!("{k} {}", v.join(" "))
            }
        })
        .collect::<Vec<_>>();

    Some(format!("{pixel_chars} {}", new_entries.join(" ")))
}

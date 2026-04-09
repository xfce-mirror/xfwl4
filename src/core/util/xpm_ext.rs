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

    let symbolic_color = entries
        .iter()
        .find(|(k, _)| *k == "s")
        .and_then(|(_, v)| v.first().copied())
        .and_then(|name| color_symbols.get(name))
        .map(|rgba| {
            format!(
                "#{:02X}{:02X}{:02X}",
                (rgba.red() * 255.0).round() as u8,
                (rgba.green() * 255.0).round() as u8,
                (rgba.blue() * 255.0).round() as u8,
            )
        });

    let c_color_value = entries
        .iter()
        .find(|(k, _)| *k == "c")
        .map(|(_, v)| v.join(" ").to_ascii_lowercase());

    let fixup_color = c_color_value
        .as_deref()
        .and_then(|name| XPM_COLOR_FIXUPS.iter().find(|(n, _)| *n == name))
        .map(|(_, hex)| (*hex).to_owned());

    let replacement = symbolic_color.or(fixup_color)?;

    let new_entries = entries
        .iter()
        .map(|(k, v)| {
            if *k == "c" {
                format!("{k} {replacement}")
            } else {
                format!("{k} {}", v.join(" "))
            }
        })
        .collect::<Vec<_>>();

    Some(format!("{pixel_chars} {}", new_entries.join(" ")))
}

// X11 color names that gdk-pixbuf either interprets differently or doesn't
// recognize at all.  xfwm4 uses the X11 color database for its custom XPM
// parser; we substitute these with hex values before passing to gdk-pixbuf.
const XPM_COLOR_FIXUPS: &[(&str, &str)] = &[
    ("alice blue", "#F0F8FF"),
    ("antique white", "#FAEBD7"),
    ("blanched almond", "#FFEBCD"),
    ("blue violet", "#8A2BE2"),
    ("cadet blue", "#5F9EA0"),
    ("cornflower blue", "#6495ED"),
    ("dark blue", "#00008B"),
    ("dark cyan", "#008B8B"),
    ("dark goldenrod", "#B8860B"),
    ("dark gray", "#A9A9A9"),
    ("dark green", "#006400"),
    ("dark grey", "#A9A9A9"),
    ("dark khaki", "#BDB76B"),
    ("dark magenta", "#8B008B"),
    ("dark olive green", "#556B2F"),
    ("dark orange", "#FF8C00"),
    ("dark orchid", "#9932CC"),
    ("dark red", "#8B0000"),
    ("dark salmon", "#E9967A"),
    ("dark sea green", "#8FBC8F"),
    ("dark slate blue", "#483D8B"),
    ("dark slate gray", "#2F4F4F"),
    ("dark slate grey", "#2F4F4F"),
    ("dark turquoise", "#00CED1"),
    ("dark violet", "#9400D3"),
    ("deep pink", "#FF1493"),
    ("deep sky blue", "#00BFFF"),
    ("dim gray", "#696969"),
    ("dim grey", "#696969"),
    ("dodger blue", "#1E90FF"),
    ("floral white", "#FFFAF0"),
    ("forest green", "#228B22"),
    ("ghost white", "#F8F8FF"),
    ("gray", "#BEBEBE"),
    ("green", "#00FF00"),
    ("green yellow", "#ADFF2F"),
    ("grey", "#BEBEBE"),
    ("hot pink", "#FF69B4"),
    ("indian red", "#CD5C5C"),
    ("lavender blush", "#FFF0F5"),
    ("lawn green", "#7CFC00"),
    ("lemon chiffon", "#FFFACD"),
    ("light blue", "#ADD8E6"),
    ("light coral", "#F08080"),
    ("light cyan", "#E0FFFF"),
    ("light goldenrod", "#EEDD82"),
    ("light goldenrod yellow", "#FAFAD2"),
    ("light gray", "#D3D3D3"),
    ("light green", "#90EE90"),
    ("light grey", "#D3D3D3"),
    ("light pink", "#FFB6C1"),
    ("light salmon", "#FFA07A"),
    ("light sea green", "#20B2AA"),
    ("light sky blue", "#87CEFA"),
    ("light slate blue", "#8470FF"),
    ("light slate gray", "#778899"),
    ("light slate grey", "#778899"),
    ("light steel blue", "#B0C4DE"),
    ("light yellow", "#FFFFE0"),
    ("lime green", "#32CD32"),
    ("maroon", "#B03060"),
    ("medium aquamarine", "#66CDAA"),
    ("medium blue", "#0000CD"),
    ("medium orchid", "#BA55D3"),
    ("medium purple", "#9370DB"),
    ("medium sea green", "#3CB371"),
    ("medium slate blue", "#7B68EE"),
    ("medium spring green", "#00FA9A"),
    ("medium turquoise", "#48D1CC"),
    ("medium violet red", "#C71585"),
    ("midnight blue", "#191970"),
    ("mint cream", "#F5FFFA"),
    ("misty rose", "#FFE4E1"),
    ("navajo white", "#FFDEAD"),
    ("navy blue", "#000080"),
    ("old lace", "#FDF5E6"),
    ("olive drab", "#6B8E23"),
    ("orange red", "#FF4500"),
    ("pale goldenrod", "#EEE8AA"),
    ("pale green", "#98FB98"),
    ("pale turquoise", "#AFEEEE"),
    ("pale violet red", "#DB7093"),
    ("papaya whip", "#FFEFD5"),
    ("peach puff", "#FFDAB9"),
    ("powder blue", "#B0E0E6"),
    ("purple", "#A020F0"),
    ("rosy brown", "#BC8F8F"),
    ("royal blue", "#4169E1"),
    ("saddle brown", "#8B4513"),
    ("sandy brown", "#F4A460"),
    ("sea green", "#2E8B57"),
    ("sky blue", "#87CEEB"),
    ("slate blue", "#6A5ACD"),
    ("slate gray", "#708090"),
    ("slate grey", "#708090"),
    ("spring green", "#00FF7F"),
    ("steel blue", "#4682B4"),
    ("violet red", "#D02090"),
    ("white smoke", "#F5F5F5"),
    ("yellow green", "#9ACD32"),
];

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gtk::gdk;

    use super::substitute_color_line;

    fn empty_symbols() -> HashMap<String, gdk::RGBA> {
        HashMap::new()
    }

    fn symbols_with(name: &str, r: f64, g: f64, b: f64) -> HashMap<String, gdk::RGBA> {
        let mut map = HashMap::new();
        map.insert(name.to_owned(), gdk::RGBA::new(r, g, b, 1.0));
        map
    }

    #[test]
    fn no_substitution_for_hex_color() {
        assert_eq!(substitute_color_line(". c #FF0000", 1, &empty_symbols()), None);
    }

    #[test]
    fn no_substitution_for_normal_named_color() {
        assert_eq!(substitute_color_line(". c red", 1, &empty_symbols()), None);
    }

    #[test]
    fn no_substitution_for_none() {
        assert_eq!(substitute_color_line("  c None", 2, &empty_symbols()), None);
    }

    #[test]
    fn symbolic_substitution() {
        let symbols = symbols_with("active_color_1", 0.5, 0.75, 1.0);
        assert_eq!(
            substitute_color_line(". c #000000 s active_color_1", 1, &symbols),
            Some(". c #80BFFF s active_color_1".to_owned()),
        );
    }

    #[test]
    fn symbolic_takes_precedence_over_fixup() {
        let symbols = symbols_with("active_color_1", 1.0, 0.0, 0.0);
        assert_eq!(
            substitute_color_line(". c gray s active_color_1", 1, &symbols),
            Some(". c #FF0000 s active_color_1".to_owned()),
        );
    }

    #[test]
    fn fixup_single_word_color() {
        assert_eq!(
            substitute_color_line("% c gray", 1, &empty_symbols()),
            Some("% c #BEBEBE".to_owned()),
        );
    }

    #[test]
    fn fixup_single_word_color_case_insensitive() {
        assert_eq!(
            substitute_color_line("% c Gray", 1, &empty_symbols()),
            Some("% c #BEBEBE".to_owned()),
        );
    }

    #[test]
    fn fixup_two_word_color() {
        assert_eq!(
            substitute_color_line(". c dark gray", 1, &empty_symbols()),
            Some(". c #A9A9A9".to_owned()),
        );
    }

    #[test]
    fn fixup_three_word_color() {
        assert_eq!(
            substitute_color_line(". c light goldenrod yellow", 1, &empty_symbols()),
            Some(". c #FAFAD2".to_owned()),
        );
    }

    #[test]
    fn fixup_multi_cpp() {
        assert_eq!(
            substitute_color_line(".. c dark slate grey", 2, &empty_symbols()),
            Some(".. c #2F4F4F".to_owned()),
        );
    }

    #[test]
    fn fixup_green() {
        assert_eq!(
            substitute_color_line(". c green", 1, &empty_symbols()),
            Some(". c #00FF00".to_owned()),
        );
    }

    #[test]
    fn fixup_maroon() {
        assert_eq!(
            substitute_color_line(". c maroon", 1, &empty_symbols()),
            Some(". c #B03060".to_owned()),
        );
    }

    #[test]
    fn fixup_purple() {
        assert_eq!(
            substitute_color_line(". c purple", 1, &empty_symbols()),
            Some(". c #A020F0".to_owned()),
        );
    }
}

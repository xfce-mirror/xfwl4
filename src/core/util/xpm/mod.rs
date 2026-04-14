// This code is taken from image-extras
// https://github.com/image-rs/image-extras/tree/0b12bd9acf392f9f2d9242ca66f316ff47b97603
//
// Copyright (C) mstoeckl <code@mstoeckl.com>
//
// MIT License
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Decoding of XPM Images
//!
//! XPM (X PixMap) Format is a plain text image format, originally designed to store
//! cursor and icon data. XPM images are valid C code.
//!
//! (This format is obsolete and nobody should make new images in it. If you need to
//! include an image in a C program, use `xxd -i` or #embed.)
//!
//! The XPM format allows for encoding an image which can be expressed differently
//! depending on the display capabilities (X11 visual), providing specialized versions
//! for color, grayscale, black and white, etc. output in the same image. In practice,
//! most XPM images created after the mid 1990s only provide a variant for the color
//! visual. As a result, this decoder implementation only outputs the color version
//! of the input image.
//!
//! A number of features of the original libXpm are not supported (because they appear to very
//! rarely have been used):
//! - XPMEXT extensions
//! - HSV color specifications
//! - Output for non-color visuals
//! - More relaxed header comment parsing (allowing different whitespace around `XPM` in `/* XPM */`)
//! - Loading with a different color table
//!
//! This is a somewhat strict decoder and will reject many broken image files, including:
//! - those using the XPM2 header or `static char ** name = {` array string
//! - those missing a trailing "," on lines, or which use ";" instead of ","
//! - those with color data lines that are too long
//! - those which have content after the final semicolon which is not a C comment
//!
//! Note: color values for the X11 color name table were _changed_ for the X11R4 release
//! in Dec 1989; since then there have only been additions.
//!
//! This overlaps with XPM version development: XPMv1 in Feb 1989, XPMv2 in Feb-August 1990,
//! and XPMv3 in April 1991. Therefore, if you _do_ see an ancient XPMv1 or XPMv2 file
//! somewhere, it may be using different color name values.
//!
//! This decoder uses the X11 color name table as of X11R6 (May 1994); the only additions since
//! then, in 2014 to add some CSS color names, are _not_ included, to preserve compatibility
//! with other XPM parsers.
//!
//! # Related Links
//! * <https://www.x.org/docs/XPM/xpm.pdf> - XPM Manual version 3.4i, which specifies the format
//! * <https://web.archive.org/web/20060702022929/http://koala.ilog.fr/ftp/pub/xpm/xpm-3-paper.ps.gz> - XPM Paper
//! * <https://en.wikipedia.org/wiki/X_PixMap> - The XPM format on wikipedia
//! * <https://web.archive.org/web/20110513234507/https://www.w3.org/People/danield/xpm_story.html> - XPM format history
//! * <https://gitlab.freedesktop.org/xorg/app/rgb/raw/master/rgb.txt> - X color names
//! * <https://www.x.org/wiki/X11R4/#index10h4> - Introduction of modern X11 color name table
//! * <https://web.archive.org/web/20070808230118/http://koala.ilog.fr/ftp/pub/xpm/> - more historical XPM material

mod x11r6colors;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Bytes};

use image::error::{DecodingError, ImageError, ImageFormatHint, ImageResult, LimitError, LimitErrorKind};
use image::{ColorType, ImageDecoder, LimitSupport, Limits};

/// Maximum length of an X11/CSS/etc. color name is 20; and of an RGB color is 13
const MAX_COLOR_NAME_LEN: usize = 32;

/// Location of a byte in the input stream.
///
/// Includes byte offset (for format debugging with hex editor) and
/// line:column offset (for format debugging with text editor)
#[derive(Clone, Copy, Debug)]
struct TextLocation {
    byte: u64,
    line: u64,
    column: u64,
}

/// A peekable reader which tracks location information
struct TextReader<R> {
    inner: R,

    current: Option<u8>,

    location: TextLocation,
}

impl<R> TextReader<R>
where
    R: Iterator<Item = u8>,
{
    /// Initialize a TextReader
    fn new(mut r: R) -> TextReader<R> {
        let current = r.next();
        TextReader {
            inner: r,
            current,
            location: TextLocation {
                byte: 0,
                line: 1,
                column: 0,
            },
        }
    }

    /// Consume the next byte. On EOF, will return None
    fn next(&mut self) -> Option<u8> {
        self.current?;

        let mut current = self.inner.next();
        std::mem::swap(&mut self.current, &mut current);

        self.location.byte += 1;
        self.location.column += 1;
        if let Some(b'\n') = current {
            self.location.line += 1;
            self.location.column = 0;
        }
        current
    }
    /// Peek at the next byte. On EOF, will return None
    fn peek(&self) -> Option<u8> {
        self.current
    }
    /// The location of the last byte returned by [Self::next]
    fn loc(&self) -> TextLocation {
        self.location
    }
}

/// Helper struct to project BufRead down to Iterator<Item=u8>. Costs of this simple
/// lifetime-free abstraction include that the struct requires space to store the
/// error value, and that code using this must eventually check the error field.
struct IoAdapter<R> {
    reader: Bytes<R>,
    error: Option<std::io::Error>,
}

impl<R> Iterator for IoAdapter<R>
where
    R: BufRead,
{
    type Item = u8;
    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.error.is_some() {
            return None;
        }
        match self.reader.next() {
            None => None,
            Some(Ok(v)) => Some(v),
            Some(Err(e)) => {
                self.error = Some(e);
                None
            }
        }
    }
}

/// XPM decoder
pub struct XpmDecoder<R> {
    r: TextReader<IoAdapter<R>>,
    info: XpmHeaderInfo,
    color_symbols: HashMap<String, [u16; 4]>,
}

/// Key XPM file properties determined from first line
struct XpmHeaderInfo {
    width: u32,
    height: u32,
    ncolors: u32,
    /// characters per pixel
    cpp: u32,
}

/// XPM color palette storage
struct XpmPalette {
    /// Sorted table of color code entries. There are many possible ways to store
    /// this, and the fastest approach depends on the image structure, number of pixels,
    /// and number of colors. While not as efficient to construct as an unsorted list,
    /// or as efficient to look values up in as a perfect hash table, the sorted table
    /// performs decently well as long as the palette is small enough to fit in CPU caches.
    table: Vec<XpmColorCodeEntry>,
    /// Color to use when a pixel references an undeclared code.  Matches xfwm4's
    /// "bad XPM...punt" behavior, which uses the first color declared in the palette.
    fallback: [u16; 4],
}

/// Pixel code and value read from the Colors section of an XPM file
struct XpmColorCodeEntry {
    code: u64,
    /// channel order: R,G,B,A
    value: [u16; 4],
}

#[derive(Debug, Clone, Copy)]
enum XpmPart {
    Header,
    ArrayStart,
    FirstLine,
    Palette,
    Body,
    Trailing,
    AfterEnd,
}

#[derive(Debug)]
enum XpmDecodeError {
    Parse(XpmPart, TextLocation),
    ZeroWidth,
    ZeroHeight,
    ZeroColors,
    BadCharsPerColor(u32),
    // A color with the given name is not available.
    // Name provided in buffer, length format, and should be alphanumeric ASCII
    UnknownColor(([u8; MAX_COLOR_NAME_LEN], u8)),
    // Palette entry is missing 'c'-type color specification
    NoColorModeColorSpecified,
    BadHexColor,
    DuplicateCode,
    TwoKeysInARow,
    MissingEntry,
    MissingColorAfterKey,
    MissingKeyBeforeColor,
    InvalidColorName,
    ColorNameTooLong,
}

/// XPM visual type keys.  Discriminants match the numeric key values used by
/// xfwm4's `xpm_extract_color`; when a palette entry specifies colors for
/// multiple visuals, the one with the greatest key value wins.  Note that
/// `Mono` effectively never wins on its own because xfwm4 initializes its
/// running `current_key` to the same value and the comparison is strict `>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum XpmVisual {
    Mono = 1,
    Grayscale4 = 2,
    Grayscale = 3,
    Color = 4,
    Symbolic = 5,
}

impl fmt::Display for TextLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("byte={},line={}:col={}", self.byte, self.line, self.column))
    }
}

impl fmt::Display for XpmPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => f.write_str("header"),
            Self::ArrayStart => f.write_str("array definition"),
            Self::FirstLine => f.write_str("<Values> section"),
            Self::Palette => f.write_str("<Colors> section"),
            Self::Body => f.write_str("<Pixels> section"),
            Self::Trailing => f.write_str("array end"),
            Self::AfterEnd => f.write_str("after final semicolon"),
        }
    }
}

impl fmt::Display for XpmDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(part, loc) => f.write_fmt(format_args!("Failed to parse {}, at {}", part, loc)),
            Self::ZeroWidth => f.write_str("Invalid (zero) image width"),
            Self::ZeroHeight => f.write_str("Invalid (zero) image height"),
            Self::ZeroColors => f.write_str("Invalid (zero) number of colors"),
            Self::BadCharsPerColor(c) => f.write_fmt(format_args!("Invalid number of characters per color: {} is not in [1,8]", c)),
            Self::UnknownColor((buf, len)) => {
                let s = std::str::from_utf8(&buf[..*len as usize]).ok().unwrap_or("");
                assert!(s.chars().all(|x| x.is_ascii_alphanumeric()));
                f.write_fmt(format_args!("Unknown color name \"{}\"; is not an X11R6 color.", s))
            }
            Self::NoColorModeColorSpecified => f.write_str("Color entry has no specified value for color visual"),
            Self::BadHexColor => f.write_str("Invalid hex RGB color"),
            Self::DuplicateCode => f.write_str("Duplicate color code"),

            Self::ColorNameTooLong => f.write_str("Invalid color name, too long"),
            Self::TwoKeysInARow => f.write_str("Invalid color specification, two keys in a row"),
            Self::MissingEntry => f.write_str("Invalid color specification, must contain at least one key-color pair"),
            Self::MissingColorAfterKey => f.write_str("Invalid color specification, no color name after key"),
            Self::MissingKeyBeforeColor => {
                f.write_str("Invalid color specification, no key before color name or could not parse value as key (m|s|g4|g|c)")
            }
            Self::InvalidColorName => f.write_str("Invalid color name, contains non-alphanumeric or non-whitespace characters"),
        }
    }
}

impl std::error::Error for XpmDecodeError {}

impl From<XpmDecodeError> for ImageError {
    fn from(e: XpmDecodeError) -> ImageError {
        ImageError::Decoding(DecodingError::new(ImageFormatHint::Name("XPM".into()), e))
    }
}

/// Helper trait for the pattern in which, after calling a function returning a Result,
/// one wishes to use an error from a different source.
trait XpmDecoderIoInjectionExt {
    type Value;
    fn apply_after(self, err: &mut Option<std::io::Error>) -> Result<Self::Value, ImageError>;
}

impl<X> XpmDecoderIoInjectionExt for Result<X, XpmDecodeError> {
    type Value = X;
    fn apply_after(self, err: &mut Option<std::io::Error>) -> Result<Self::Value, ImageError> {
        if let Some(err) = err.take() {
            return Err(ImageError::IoError(err));
        }
        match self {
            Self::Ok(x) => Ok(x),
            Self::Err(e) => Err(e.into()),
        }
    }
}

/// Is x a valid character to use in a word of a color name
fn valid_name_char(x: u8) -> bool {
    // underscore: used in some symbolic names
    matches!(x, b'#' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_')
}
/// Replace upper case by lower case ASCII letters
fn fold_to_lower(x: u8) -> u8 {
    match x {
        b'A'..=b'Z' => (x - b'A') + b'a',
        _ => x,
    }
}

/// Read a C keyword into the buffer and returns a slice of the buffer for the
/// keyword.
///
/// The only allowed characters are a-z, A-Z, and _. Reading will stop if
/// a non-allowed character or EOF is reached. If the buffer is too small, an
/// error will be returned.
fn read_keyword<'buf, R: Iterator<Item = u8>>(
    r: &mut TextReader<R>,
    buf: &'buf mut [u8],
    part: XpmPart,
) -> Result<&'buf [u8], XpmDecodeError> {
    let mut len = 0;

    while let Some(b) = r.peek() {
        if matches!(b, b'_' | b'a'..=b'z' | b'A'..=b'Z') {
            if len >= buf.len() {
                // identifier too long
                return Err(XpmDecodeError::Parse(part, r.loc()));
            }
            buf[len] = b;
            len += 1;
            r.next();
        } else {
            break;
        }
    }

    Ok(&buf[..len])
}
/// Read precisely the string `s` from `r`, or error.
fn read_fixed_string<R: Iterator<Item = u8>>(r: &mut TextReader<R>, s: &[u8], part: XpmPart) -> Result<(), XpmDecodeError> {
    for c in s {
        if let Some(b) = r.next() {
            if b != *c {
                return Err(XpmDecodeError::Parse(part, r.loc()));
            }
        } else {
            return Err(XpmDecodeError::Parse(part, r.loc()));
        };
    }
    Ok(())
}
// Read a single byte
fn read_byte<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<u8, XpmDecodeError> {
    match r.next() {
        None => Err(XpmDecodeError::Parse(part, r.loc())),
        Some(b) => Ok(b),
    }
}

/// Read a mixture of ' ' and '\t'. At least one character must be read.
// Other whitespace characters are not permitted.
fn read_whitespace_gap<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<(), XpmDecodeError> {
    let b = read_byte(r, part)?;
    if !(b == b' ' || b == b'\t') {
        return Err(XpmDecodeError::Parse(part, r.loc()));
    }
    while let Some(b) = r.peek() {
        if b == b' ' || b == b'\t' {
            r.next();
            continue;
        } else {
            return Ok(());
        }
    }
    Ok(())
}

/// Read a mixture of ' ', '\t', '\n', and C-style /* comments */.
/// This will error if it sees a / without following *
fn skip_whitespace_and_comments<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<usize, XpmDecodeError> {
    let mut nbytes = 0;

    // `has_first_char`: If out of comment, has / ; if in comment, has *
    let mut has_first_char = false;
    let mut in_comment = false;

    while let Some(b) = r.peek() {
        if !in_comment {
            if has_first_char {
                if b != b'*' {
                    return Err(XpmDecodeError::Parse(part, r.loc()));
                } else {
                    in_comment = true;
                    has_first_char = false;
                }
            }
            if b == b'/' {
                has_first_char = true;
            }
        }
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'/' || in_comment {
            if in_comment {
                if has_first_char && b == b'/' {
                    in_comment = false;
                }
                has_first_char = b == b'*';
            }
            nbytes += 1;
            r.next();
            continue;
        } else {
            break;
        }
    }
    if !in_comment && has_first_char {
        // Parsed up to a / but did not find *
        return Err(XpmDecodeError::Parse(part, r.loc()));
    }

    Ok(nbytes)
}

/// Skips at least one whitespace or comment.
fn skip_non_empty_whitespace_and_comments<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<(), XpmDecodeError> {
    let spaces = skip_whitespace_and_comments(r, part)?;
    if spaces == 0 {
        return Err(XpmDecodeError::Parse(part, r.loc()));
    }
    Ok(())
}

fn skip_spaces_and_tabs<R: Iterator<Item = u8>>(r: &mut TextReader<R>) -> Result<usize, XpmDecodeError> {
    let mut nbytes = 0;
    while let Some(b) = r.peek() {
        if b == b' ' || b == b'\t' {
            nbytes += 1;
            r.next();
            continue;
        } else {
            break;
        }
    }
    Ok(nbytes)
}

/// Read a mixture of ' ' and '\t', until reading '\n'.
fn read_to_newline<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<(), XpmDecodeError> {
    while let Some(b) = r.peek() {
        if b == b' ' || b == b'\t' {
            r.next();
            continue;
        } else {
            break;
        }
    }
    if read_byte(r, part)? != b'\n' {
        Err(XpmDecodeError::Parse(part, r.loc()))
    } else {
        Ok(())
    }
}
/// Read token into the buffer until the buffer size is exceeded, or ' ' or '\t' or '"' is found
/// \ characters are forbidden. Returns the region of data read.
fn read_until_whitespace_or_eos<'a, R: Iterator<Item = u8>>(
    r: &mut TextReader<R>,
    buf: &'a mut [u8],
    part: XpmPart,
) -> Result<&'a mut [u8], XpmDecodeError> {
    let mut len = 0;
    while let Some(b) = r.peek() {
        if b == b' ' || b == b'\t' || b == b'"' {
            return Ok(&mut buf[..len]);
        } else if b == b'\\' {
            r.next();
            return Err(XpmDecodeError::Parse(part, r.loc()));
        } else {
            if len >= buf.len() {
                // identifier is too long
                return Err(XpmDecodeError::Parse(part, r.loc()));
            }
            buf[len] = b;
            len += 1;
            r.next();
        }
    }
    Ok(&mut buf[..len])
}

/// Read fixed length token into the buffer. Errors if file ends, or " or \ is found.
fn read_all_except_eos<R: Iterator<Item = u8>>(r: &mut TextReader<R>, buf: &mut [u8], part: XpmPart) -> Result<(), XpmDecodeError> {
    let mut len = 0;
    while let Some(b) = r.peek() {
        if b == b'"' || b == b'\\' {
            r.next();
            return Err(XpmDecodeError::Parse(part, r.loc()));
        } else {
            buf[len] = b;
            len += 1;
            r.next();
            if len >= buf.len() {
                return Ok(());
            }
        }
    }
    Err(XpmDecodeError::Parse(part, r.loc()))
}

/// Read the name portion of the file (but do not validate it, because some old files
/// may put invalid characters here (like "." and "-") or use 8-bit character sets instead
/// of Unicode.)
fn read_name<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<(), XpmDecodeError> {
    let mut empty = true;
    while let Some(b) = r.peek() {
        match b {
            b'/' | b' ' | b'\t' | b'\n' | b'[' => {
                break;
            }
            _ => (),
        }
        r.next();
        empty = false;
    }
    if empty {
        return Err(XpmDecodeError::Parse(part, r.loc()));
    }

    Ok(())
}

/// Parse string into integer, rejecting leading + and leading zeros
fn parse_i32(data: &[u8]) -> Option<i32> {
    if data.starts_with(b"-") {
        (-(parse_u32(&data[1..])? as i64)).try_into().ok()
    } else {
        parse_u32(data)?.try_into().ok()
    }
}

/// Parse string into unsigned integer, rejecting leading + and leading zeros
fn parse_u32(data: &[u8]) -> Option<u32> {
    let Some(c1) = data.first() else {
        // Reject empty string
        return None;
    };
    if *c1 == b'0' && data.len() > 1 {
        // Reject leading zeros unless value is exactly zero
        return None;
    }
    let mut x: u32 = 0;
    for c in data {
        if b'0' <= *c && *c <= b'9' {
            x = x.checked_mul(10)?.checked_add((*c - b'0') as u32)?;
        } else {
            return None;
        }
    }
    Some(x)
}
fn parse_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}
fn parse_hex1(x1: u8) -> Option<u16> {
    let x = parse_hex(x1)? as u16;
    Some(x | (x << 4) | (x << 8) | (x << 12))
}
fn parse_hex2(x2: u8, x1: u8) -> Option<u16> {
    let x = ((parse_hex(x2)? as u16) << 4) | (parse_hex(x1)? as u16);
    Some(x | (x << 8))
}
fn parse_hex3(x3: u8, x2: u8, x1: u8) -> Option<u16> {
    let x = ((parse_hex(x3)? as u16) << 8) | ((parse_hex(x2)? as u16) << 4) | (parse_hex(x1)? as u16);
    // There are four reasonable approaches to converting 12-bit to 16-bit,
    // round down, round nearest, round up, and round fast
    // (x*65535)/4095, (x*65535+2047)/4095, (x*65535+4094)/4095, and (x<<4)|(x>>8).
    Some((((x as u32) * 65535 + 2047) / 4095) as u16)
}
fn parse_hex4(x4: u8, x3: u8, x2: u8, x1: u8) -> Option<u16> {
    Some((parse_hex(x1)? as u16) | ((parse_hex(x2)? as u16) << 4) | ((parse_hex(x3)? as u16) << 8) | ((parse_hex(x4)? as u16) << 12))
}
fn scale_u8_to_u16(x: u8) -> u16 {
    (x as u16) << 8 | (x as u16)
}

/// Parse an #RGB-style color.
/// Note: this deviates from XParseColor in order to sensibly interpret #aabbcc as #aaaabbbbcccc
/// instead of #aa00bb00cc00.
fn parse_hex_color(data: &[u8]) -> Option<[u16; 4]> {
    Some(match data {
        [r, g, b] => [parse_hex1(*r)?, parse_hex1(*g)?, parse_hex1(*b)?, 0xffff],
        [r2, r1, g2, g1, b2, b1] => [parse_hex2(*r2, *r1)?, parse_hex2(*g2, *g1)?, parse_hex2(*b2, *b1)?, 0xffff],
        [r3, r2, r1, g3, g2, g1, b3, b2, b1] => [
            parse_hex3(*r3, *r2, *r1)?,
            parse_hex3(*g3, *g2, *g1)?,
            parse_hex3(*b3, *b2, *b1)?,
            0xffff,
        ],
        [r4, r3, r2, r1, g4, g3, g2, g1, b4, b3, b2, b1] => [
            parse_hex4(*r4, *r3, *r2, *r1)?,
            parse_hex4(*g4, *g3, *g2, *g1)?,
            parse_hex4(*b4, *b3, *b2, *b1)?,
            0xffff,
        ],
        _ => {
            return None;
        }
    })
}

fn parse_color(data: &[u8]) -> Result<[u16; 4], XpmDecodeError> {
    if data.starts_with(b"#") {
        parse_hex_color(&data[1..]).ok_or(XpmDecodeError::BadHexColor)
    } else {
        if data == b"none" {
            return Ok([0, 0, 0, 0]);
        }

        if let Ok(idx) = x11r6colors::COLORS.binary_search_by(|entry| entry.0.as_bytes().cmp(data)) {
            let entry = x11r6colors::COLORS[idx];
            Ok([scale_u8_to_u16(entry.1), scale_u8_to_u16(entry.2), scale_u8_to_u16(entry.3), 0xffff])
        } else {
            // At this point, `data` has been validated as alphanumeric ASCII; read_xpm_palette
            // should ensure its length is <= MAX_COLOR_NAME_LEN
            assert!(data.len() <= MAX_COLOR_NAME_LEN);
            let mut tmp = [0u8; MAX_COLOR_NAME_LEN];
            tmp[..data.len()].copy_from_slice(data);
            Err(XpmDecodeError::UnknownColor((tmp, data.len() as u8)))
        }
    }
}

/// Read the header of the XPM image and first line
fn read_xpm_header<R: Iterator<Item = u8>>(r: &mut TextReader<R>) -> Result<XpmHeaderInfo, XpmDecodeError> {
    let keyword_buf = &mut [0u8; 16];

    // Note: XPM3 header is `/* XPM */`
    read_fixed_string(r, b"/* XPM */", XpmPart::Header)?;
    read_to_newline(r, XpmPart::Header)?;

    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"static", XpmPart::ArrayStart)?;
    skip_non_empty_whitespace_and_comments(r, XpmPart::ArrayStart)?;

    // There may be an optional "const" keyword before "char".
    // This is NOT part of the XPM 3 specification.
    // This was added by ImageMagick 7.1.0-4 in mid 2021
    // (https://github.com/ImageMagick/ImageMagick/commit/e7d3e182b72ff9b2c3ea1c9aa0f14d69cc968ba7)
    // to help with C++ compiler warnings (https://github.com/ImageMagick/ImageMagick/issues/3951).
    let keyword = read_keyword(r, keyword_buf, XpmPart::ArrayStart)?;
    match keyword {
        b"const" => {
            skip_non_empty_whitespace_and_comments(r, XpmPart::ArrayStart)?;
            read_fixed_string(r, b"char", XpmPart::ArrayStart)?;
        }
        b"char" => (),
        _ => return Err(XpmDecodeError::Parse(XpmPart::ArrayStart, r.loc())),
    }

    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"*", XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_name(r, XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"[", XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"]", XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"=", XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;
    read_fixed_string(r, b"{", XpmPart::ArrayStart)?;
    skip_whitespace_and_comments(r, XpmPart::ArrayStart)?;

    /* next: read \" */
    read_fixed_string(r, b"\"", XpmPart::FirstLine)?;

    // Inside strings, only spaces are allowed for separators
    let mut int_buf = [0u8; 10]; // 2^32 fits in 10 bytes
    skip_spaces_and_tabs(r)?; // words separated by space & tabulation chars -- so skip both?
    let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
    let width = parse_u32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
    if width == 0 {
        return Err(XpmDecodeError::ZeroWidth);
    }

    read_whitespace_gap(r, XpmPart::FirstLine)?;
    let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
    let height = parse_u32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
    if height == 0 {
        return Err(XpmDecodeError::ZeroHeight);
    }

    read_whitespace_gap(r, XpmPart::FirstLine)?;
    let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
    let ncolors = parse_u32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
    read_whitespace_gap(r, XpmPart::FirstLine)?;
    let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
    let cpp = parse_u32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
    skip_spaces_and_tabs(r)?;

    let _hotspot = if let Some(b'"') = r.peek() {
        // Done
        None
    } else {
        let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
        let hotspot_x = parse_i32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
        read_whitespace_gap(r, XpmPart::FirstLine)?;
        let int = read_until_whitespace_or_eos(r, &mut int_buf, XpmPart::FirstLine)?;
        let hotspot_y = parse_i32(int).ok_or(XpmDecodeError::Parse(XpmPart::FirstLine, r.loc()))?;
        skip_spaces_and_tabs(r)?;

        // Parse hotspot now.
        Some((hotspot_x, hotspot_y))
    };
    // XPMEXT tags are not supported -- they were essentially never used in practice.

    read_fixed_string(r, b"\"", XpmPart::FirstLine)?;
    skip_whitespace_and_comments(r, XpmPart::FirstLine)?;
    read_fixed_string(r, b",", XpmPart::FirstLine)?;
    skip_whitespace_and_comments(r, XpmPart::FirstLine)?;

    if ncolors == 0 {
        return Err(XpmDecodeError::ZeroColors);
    }
    if cpp == 0 || cpp > 8 {
        /* cpp larger than 8 is pointless and would not be made by sane encoders:
         * with hex encoding, it would allow 2^32 distinct colors. */
        return Err(XpmDecodeError::BadCharsPerColor(cpp));
    }

    Ok(XpmHeaderInfo {
        width,
        height,
        ncolors,
        cpp,
    })
}
/// Read the palette portion of the XPM image, stopping just before the first pixel
fn read_xpm_palette<R: Iterator<Item = u8>>(
    r: &mut TextReader<R>,
    info: &XpmHeaderInfo,
    color_symbols: &HashMap<String, [u16; 4]>,
) -> Result<XpmPalette, XpmDecodeError> {
    assert!(1 <= info.cpp && info.cpp <= 8);

    // Check that color table is sorted
    assert!(x11r6colors::COLORS.windows(2).all(|p| p[0].0 < p[1].0));

    // Even though the file provides a value for `ncolors`, and memory limits are validated,
    // do NOT reserve the suggested memory in advance. Dynamically resizing the vector
    // is negligibly slower, but ensures that the amount of memory allocated is always
    // bounded by a multiple of the actual file size. Kernel virtual memory optimizations
    // may hide the performance cost of allocating a 100MB color table from the
    // application, but such allocations are still expensive even if mostly unused.
    let mut color_table: Vec<XpmColorCodeEntry> = Vec::new();

    for _col in 0..info.ncolors {
        read_fixed_string(r, b"\"", XpmPart::Palette)?;

        let mut code = [0_u8; 8];
        read_all_except_eos(r, &mut code[..info.cpp as usize], XpmPart::Palette)?;
        read_whitespace_gap(r, XpmPart::Palette)?;

        // Color parsing: XPM color specifications have the form {<key> <color>}+
        // This is tricky to parse correctly as color names may contain spaces.
        // Fortunately, the key values are "m", "s", "g4", "g", "c", which will
        // never be a word within a color name, so one can acquire the entire color
        // name by parsing until the next key appears or until '"' arrives.

        // Like the X server, this parser does a case-insensitive match on color names.
        // Unfortunately, there is no general way to handle spaces in names: the color
        // name database includes variants with spaces for multi-word names that do not
        // end in a number; e.g. "antiquewhite" has a split variation "antique white",
        // but "antiquewhite3" does not.

        let mut color_name_buf = [0_u8; MAX_COLOR_NAME_LEN];
        let mut color_name_len = 0;
        let mut next_buf = [0_u8; MAX_COLOR_NAME_LEN];

        let mut key: Option<XpmVisual> = None;

        let mut best_color: Option<(XpmVisual, [u16; 4])> = None;
        let mut symbolic: Option<([u8; MAX_COLOR_NAME_LEN], usize)> = None;
        let apply_segment = |k: XpmVisual,
                             name: &[u8],
                             best_color: &mut Option<(XpmVisual, [u16; 4])>,
                             symbolic: &mut Option<([u8; MAX_COLOR_NAME_LEN], usize)>|
         -> Result<(), XpmDecodeError> {
            match handle_key_color(k, name)? {
                KeyColor::Color { visual, value } => {
                    if best_color.is_none_or(|(best, _)| visual > best) {
                        *best_color = Some((visual, value));
                    }
                }
                KeyColor::Symbolic { buf, len } => *symbolic = Some((buf, len)),
            }
            Ok(())
        };
        loop {
            if r.peek().unwrap_or(b'"') == b'"' {
                let Some(k) = key else {
                    // At end of line, must have read a key
                    return Err(XpmDecodeError::MissingEntry);
                };
                if color_name_len == 0 {
                    // At end of line, must also have read a color to process
                    return Err(XpmDecodeError::MissingColorAfterKey);
                }

                apply_segment(k, &color_name_buf[..color_name_len], &mut best_color, &mut symbolic)?;
                break;
            }

            let next = read_until_whitespace_or_eos(r, &mut next_buf, XpmPart::Palette)?;
            skip_spaces_and_tabs(r)?;

            let this_key = match &next[..] {
                b"m" => Some(XpmVisual::Mono),
                b"s" => Some(XpmVisual::Symbolic),
                b"g4" => Some(XpmVisual::Grayscale4),
                b"g" => Some(XpmVisual::Grayscale),
                b"c" => Some(XpmVisual::Color),
                _ => None,
            };

            let Some(k) = key else {
                // No key has been set, is first key-color pair in the line
                if this_key.is_none() {
                    // Error: processing non-key value with no preceding key
                    return Err(XpmDecodeError::MissingKeyBeforeColor);
                };

                key = this_key;
                continue;
            };

            if this_key.is_some() {
                // End of preceding segment
                if color_name_len == 0 {
                    return Err(XpmDecodeError::TwoKeysInARow);
                }

                apply_segment(k, &color_name_buf[..color_name_len], &mut best_color, &mut symbolic)?;
                color_name_len = 0;
                key = this_key;
                continue;
            }

            // Validate word, case fold it, and concatenate it with the preceding word,
            // adding a space betweeen words
            if color_name_len > 0 {
                if color_name_len < MAX_COLOR_NAME_LEN {
                    color_name_buf[color_name_len] = b' ';
                    color_name_len += 1;
                } else {
                    return Err(XpmDecodeError::ColorNameTooLong);
                }
            }
            for c in next {
                if !valid_name_char(*c) {
                    return Err(XpmDecodeError::InvalidColorName);
                }
                // Reduce to lowercase, matching the color name database, to
                // make regular string comparisons be case-insensitive
                if color_name_len < MAX_COLOR_NAME_LEN {
                    color_name_buf[color_name_len] = fold_to_lower(*c);
                    color_name_len += 1;
                } else {
                    return Err(XpmDecodeError::ColorNameTooLong);
                }
            }
        }

        let themed_color = symbolic
            .as_ref()
            .and_then(|(buf, len)| std::str::from_utf8(&buf[..*len]).ok())
            .and_then(|name| color_symbols.get(name))
            .copied();
        let Some(color) = themed_color.or_else(|| best_color.map(|(_, c)| c)) else {
            return Err(XpmDecodeError::NoColorModeColorSpecified);
        };

        color_table.push(XpmColorCodeEntry {
            code: u64::from_le_bytes(code),
            value: color,
        });

        read_fixed_string(r, b"\"", XpmPart::Palette)?;
        skip_whitespace_and_comments(r, XpmPart::Palette)?;
        read_fixed_string(r, b",", XpmPart::Palette)?;
        skip_whitespace_and_comments(r, XpmPart::Palette)?;
    }

    // xfwm4 uses the first color declared in the palette as the fallback for
    // undeclared pixel codes ("Bad XPM...punt").  Capture it before sorting.
    let fallback = color_table.first().map(|entry| entry.value).unwrap_or([0, 0, 0, 0]);

    // Sort table and check for duplicates
    color_table.sort_unstable_by_key(|x| x.code);
    for w in color_table.windows(2) {
        if w[0].code.cmp(&w[1].code) != Ordering::Less {
            return Err(XpmDecodeError::DuplicateCode);
        }
    }

    read_fixed_string(r, b"\"", XpmPart::Body)?;

    Ok(XpmPalette {
        table: color_table,
        fallback,
    })
}
/// Read a single pixel from within the main image area.  Returns `Ok(true)`
/// if a pixel was read, or `Ok(false)` if the closing `"` of the current row
/// was reached before a full pixel code could be read.  Rows that are shorter
/// than the declared width are silently accepted (matching xfwm4).
fn read_xpm_pixel<R: Iterator<Item = u8>>(
    r: &mut TextReader<R>,
    info: &XpmHeaderInfo,
    palette: &XpmPalette,
    chunk: &mut [u8; 8],
) -> Result<bool, XpmDecodeError> {
    let mut code = [0_u8; 8];
    let cpp = info.cpp as usize;
    for slot in code[..cpp].iter_mut() {
        match r.peek() {
            Some(b'"') => return Ok(false),
            Some(b'\\') => {
                r.next();
                return Err(XpmDecodeError::Parse(XpmPart::Body, r.loc()));
            }
            Some(b) => {
                *slot = b;
                r.next();
            }
            None => return Err(XpmDecodeError::Parse(XpmPart::Body, r.loc())),
        }
    }
    let code = u64::from_le_bytes(code);

    let color = palette
        .table
        .binary_search_by(|entry| entry.code.cmp(&code))
        .map(|index| palette.table[index].value)
        .unwrap_or(palette.fallback);
    // ColorType::Rgba16 is currently native endian, R,G,B,A channel order
    chunk[0..2].copy_from_slice(&color[0].to_ne_bytes());
    chunk[2..4].copy_from_slice(&color[1].to_ne_bytes());
    chunk[4..6].copy_from_slice(&color[2].to_ne_bytes());
    chunk[6..8].copy_from_slice(&color[3].to_ne_bytes());
    Ok(true)
}

/// Skip any pixel-data characters between the last read pixel and the closing
/// quote of the current row.  Some XPM files declare a width smaller than
/// the actual row content; libXpm, gdk-pixbuf, and xfwm4 all read exactly
/// `width * cpp` pixel codes per row and ignore the rest, so match that.
fn skip_row_padding<R: Iterator<Item = u8>>(r: &mut TextReader<R>, part: XpmPart) -> Result<(), XpmDecodeError> {
    while let Some(b) = r.peek() {
        if b == b'"' || b == b'\\' {
            break;
        }
        r.next();
    }
    if r.peek() != Some(b'"') {
        Err(XpmDecodeError::Parse(part, r.loc()))
    } else {
        Ok(())
    }
}

/// Read the end of this row of the XPM image body and the start of the next.
/// Should only be called between rows, and not after the last one
fn read_xpm_row_transition<R: Iterator<Item = u8>>(r: &mut TextReader<R>) -> Result<(), XpmDecodeError> {
    skip_row_padding(r, XpmPart::Body)?;
    // End of this line
    read_fixed_string(r, b"\"", XpmPart::Body)?;

    skip_whitespace_and_comments(r, XpmPart::Body)?;
    read_fixed_string(r, b",", XpmPart::Body)?;
    skip_whitespace_and_comments(r, XpmPart::Body)?;
    // Start of next line
    read_fixed_string(r, b"\"", XpmPart::Body)?;
    Ok(())
}
/// Read the end of the XPM image
fn read_xpm_trailing<R: Iterator<Item = u8>>(r: &mut TextReader<R>) -> Result<(), XpmDecodeError> {
    skip_row_padding(r, XpmPart::Body)?;
    // Read end of last line
    read_fixed_string(r, b"\"", XpmPart::Body)?;

    // Some XPM files declare a smaller height than the number of pixel rows
    // they actually contain.  Match libXpm/gdk-pixbuf/xfwm4 by ignoring any
    // extra string literals between the end of the declared rows and the
    // closing `};`.
    loop {
        skip_whitespace_and_comments(r, XpmPart::Trailing)?;
        match r.peek() {
            Some(b',') => {
                r.next();
                skip_whitespace_and_comments(r, XpmPart::Trailing)?;
            }
            Some(b'}') => {
                r.next();
                break;
            }
            Some(b'"') => {
                r.next();
                skip_row_padding(r, XpmPart::Trailing)?;
                read_fixed_string(r, b"\"", XpmPart::Trailing)?;
            }
            _ => return Err(XpmDecodeError::Parse(XpmPart::Trailing, r.loc())),
        }
    }
    skip_whitespace_and_comments(r, XpmPart::Trailing)?;
    read_fixed_string(r, b";", XpmPart::Trailing)?;

    skip_whitespace_and_comments(r, XpmPart::AfterEnd)?;
    if r.next().is_some() {
        // File has unexpected trailing contents.
        Err(XpmDecodeError::Parse(XpmPart::AfterEnd, r.loc()))
    } else {
        Ok(())
    }
}

impl<R> XpmDecoder<R>
where
    R: BufRead,
{
    /// Create a new [XpmDecoder] with a table of symbolic color names.
    ///
    /// XPM color entries may use the `s` (symbolic) key to declare a named color that can be
    /// resolved at load time.  If a color entry has a symbolic name that matches a key in
    /// `color_symbols`, the associated color will be used; otherwise the `c` (color) value
    /// will be used as a fallback.  Keys in `color_symbols` should be ASCII lowercase to
    /// match the case-folded symbolic names produced by the parser.
    pub fn new(reader: R, color_symbols: HashMap<String, [u16; 4]>) -> Result<XpmDecoder<R>, ImageError> {
        let mut r = TextReader::new(IoAdapter {
            reader: reader.bytes(),
            error: None,
        });

        let info = read_xpm_header(&mut r).apply_after(&mut r.inner.error)?;

        Ok(XpmDecoder { r, info, color_symbols })
    }
}

/// Parse a key/color pair.  For visual keys that carry an RGB color
/// (`m`, `g4`, `g`, `c`) returns the parsed color tagged with its visual, so
/// the caller can select the highest-keyed visual present in the entry
/// (matching xfwm4's behavior).  For the symbolic key (`s`) returns the
/// raw name, to be resolved against the theme's color symbol table.
fn handle_key_color(key: XpmVisual, color: &[u8]) -> Result<KeyColor, XpmDecodeError> {
    match key {
        XpmVisual::Symbolic => {
            let mut buf = [0u8; MAX_COLOR_NAME_LEN];
            buf[..color.len()].copy_from_slice(color);
            Ok(KeyColor::Symbolic { buf, len: color.len() })
        }
        XpmVisual::Mono | XpmVisual::Grayscale4 | XpmVisual::Grayscale | XpmVisual::Color => Ok(KeyColor::Color {
            visual: key,
            value: parse_color(color)?,
        }),
    }
}

enum KeyColor {
    Color { visual: XpmVisual, value: [u16; 4] },
    Symbolic { buf: [u8; MAX_COLOR_NAME_LEN], len: usize },
}

impl<R: BufRead> ImageDecoder for XpmDecoder<R> {
    fn dimensions(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }
    fn color_type(&self) -> ColorType {
        // note: some images specify 16-bpc colors, and fully transparent pixels are possible,
        // so RGBA16 is needed to handle all possible cases
        ColorType::Rgba16
    }
    fn read_image(mut self, buf: &mut [u8]) -> ImageResult<()>
    where
        Self: Sized,
    {
        assert!(1 <= self.info.cpp && self.info.cpp <= 8);

        let palette = read_xpm_palette(&mut self.r, &self.info, &self.color_symbols).apply_after(&mut self.r.inner.error)?;

        // Read main image contents
        let stride = (self.info.width as usize).checked_mul(8).unwrap();
        for (i, row) in buf.chunks_exact_mut(stride).enumerate() {
            for chunk in row.chunks_exact_mut(8) {
                let got_pixel =
                    read_xpm_pixel(&mut self.r, &self.info, &palette, chunk.try_into().unwrap()).apply_after(&mut self.r.inner.error)?;
                if !got_pixel {
                    // Row is shorter than declared width; leave the rest of the
                    // row as zero-initialized pixels (transparent black), matching
                    // xfwm4's "row too short → continue" behavior.
                    break;
                }
            }

            if i >= (self.info.height - 1) as usize {
                // Last row,
            } else {
                read_xpm_row_transition(&mut self.r).apply_after(&mut self.r.inner.error)?;
            }
        }

        read_xpm_trailing(&mut self.r).apply_after(&mut self.r.inner.error)?;

        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }

    fn set_limits(&mut self, limits: Limits) -> ImageResult<()> {
        limits.check_support(&LimitSupport::default())?;
        let (width, height) = self.dimensions();
        limits.check_dimensions(width, height)?;

        let max_pixels = u64::from(self.info.width) * u64::from(self.info.height);
        let max_image_bytes = max_pixels
            .checked_mul(8)
            .ok_or(ImageError::Limits(LimitError::from_kind(LimitErrorKind::DimensionError)))?;

        let max_table_bytes = (self.info.ncolors as u64) * (size_of::<XpmColorCodeEntry>() as u64);
        let max_bytes = max_image_bytes
            .checked_add(max_table_bytes)
            .ok_or(ImageError::Limits(LimitError::from_kind(LimitErrorKind::InsufficientMemory)))?;

        let max_alloc = limits.max_alloc.unwrap_or(u64::MAX);
        if max_alloc < max_bytes {
            return Err(ImageError::Limits(LimitError::from_kind(LimitErrorKind::InsufficientMemory)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_missing_body() {
        let data = b"/* XPM */
static char *test[] = {
\"20 5 10 1\",
};
";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0; decoder.total_bytes() as usize];
        assert!(decoder.read_image(&mut image).is_err());
    }

    #[test]
    fn invalid_color_name() {
        let data = b"/* XPM */
static char *test[] = {
    \"1 1 1 1\",
    \"  c Antique White1\",
    \" \",
};";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0; decoder.total_bytes() as usize];
        assert!(decoder.read_image(&mut image).is_err());
    }

    fn decode_single_pixel(data: &[u8], color_symbols: HashMap<String, [u16; 4]>) -> [u16; 4] {
        let decoder = XpmDecoder::new(data, color_symbols).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut image).unwrap();
        assert_eq!(image.len(), 8);
        [
            u16::from_ne_bytes([image[0], image[1]]),
            u16::from_ne_bytes([image[2], image[3]]),
            u16::from_ne_bytes([image[4], image[5]]),
            u16::from_ne_bytes([image[6], image[7]]),
        ]
    }

    #[test]
    fn symbolic_substitution_applies_when_matched() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". c #FF0000 s active_color_1\",
\".\",
};
";
        let mut symbols = HashMap::new();
        symbols.insert("active_color_1".to_owned(), [0x0000, 0xFFFF, 0x0000, 0xFFFF]);
        assert_eq!(decode_single_pixel(data, symbols), [0x0000, 0xFFFF, 0x0000, 0xFFFF]);
    }

    #[test]
    fn symbolic_substitution_falls_back_to_c_color() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". c #FF0000 s not_in_theme\",
\".\",
};
";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0xFFFF, 0x0000, 0x0000, 0xFFFF]);
    }

    #[test]
    fn symbolic_without_c_color_uses_theme() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". s active_color_1\",
\".\",
};
";
        let mut symbols = HashMap::new();
        symbols.insert("active_color_1".to_owned(), [0x1234, 0x5678, 0x9ABC, 0xFFFF]);
        assert_eq!(decode_single_pixel(data, symbols), [0x1234, 0x5678, 0x9ABC, 0xFFFF]);
    }

    #[test]
    fn symbolic_without_c_color_and_no_theme_match_fails() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". s not_in_theme\",
\".\",
};
";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        assert!(decoder.read_image(&mut image).is_err());
    }

    #[test]
    fn symbolic_substitution_case_folded() {
        // The parser lowercases symbolic names before lookup, so the theme keys
        // must be lowercase to match.
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". c #FF0000 s Active_Color_1\",
\".\",
};
";
        let mut symbols = HashMap::new();
        symbols.insert("active_color_1".to_owned(), [0x0000, 0x0000, 0xFFFF, 0xFFFF]);
        assert_eq!(decode_single_pixel(data, symbols), [0x0000, 0x0000, 0xFFFF, 0xFFFF]);
    }

    #[test]
    fn c_visual_wins_over_g() {
        // When both `c` and `g` are specified, the Color visual should win.
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". g #00FF00 c #FF0000\",
\".\",
};";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0xFFFF, 0x0000, 0x0000, 0xFFFF]);
    }

    #[test]
    fn g_visual_used_when_no_c() {
        // When only `g` is specified (no `c`), it should win.
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". g #00FF00\",
\".\",
};";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0x0000, 0xFFFF, 0x0000, 0xFFFF]);
    }

    #[test]
    fn g4_visual_used_when_no_c_or_g() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". g4 #0000FF\",
\".\",
};";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0x0000, 0x0000, 0xFFFF, 0xFFFF]);
    }

    #[test]
    fn g_wins_over_g4() {
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". g4 #0000FF g #00FF00\",
\".\",
};";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0x0000, 0xFFFF, 0x0000, 0xFFFF]);
    }

    #[test]
    fn visual_order_independent() {
        // Order of visuals within the entry shouldn't matter; highest key wins.
        let data = b"/* XPM */
static char *test[] = {
\"1 1 1 1\",
\". c #FF0000 g #00FF00 g4 #0000FF\",
\".\",
};";
        assert_eq!(decode_single_pixel(data, HashMap::new()), [0xFFFF, 0x0000, 0x0000, 0xFFFF]);
    }

    #[test]
    fn row_wider_than_declared_width() {
        // Some XPM files (like those in Default-hdpi) declare a width smaller
        // than the actual content of each pixel row.  The XPM decoder should
        // read only `width * cpp` pixel codes per row and ignore the rest.
        let data = b"/* XPM */
static char *test[] = {
\"2 2 2 1\",
\". c #FF0000\",
\"# c #00FF00\",
\".#XX\",
\"#.YY\",
};";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut image).unwrap();
        // 2x2 image, 4 pixels, 8 bytes each = 32 bytes total
        assert_eq!(image.len(), 32);
        // Pixel (0,0) = red
        assert_eq!(u16::from_ne_bytes([image[0], image[1]]), 0xFFFF);
        // Pixel (1,0) = green
        assert_eq!(u16::from_ne_bytes([image[8], image[9]]), 0x0000);
        assert_eq!(u16::from_ne_bytes([image[10], image[11]]), 0xFFFF);
        // Pixel (0,1) = green
        assert_eq!(u16::from_ne_bytes([image[16], image[17]]), 0x0000);
        // Pixel (1,1) = red
        assert_eq!(u16::from_ne_bytes([image[24], image[25]]), 0xFFFF);
    }

    #[test]
    fn unknown_pixel_code_uses_first_declared_color() {
        // xfwm4 uses the first declared color as fallback for undeclared pixel codes.
        // Here `X` is not in the palette; the decoder should emit the first color
        // (red) for those pixels.
        let data = b"/* XPM */
static char *test[] = {
\"2 1 2 1\",
\". c #FF0000\",
\"# c #00FF00\",
\"X.\",
};";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut image).unwrap();
        // First pixel (unknown code X) → fallback red
        assert_eq!(u16::from_ne_bytes([image[0], image[1]]), 0xFFFF);
        assert_eq!(u16::from_ne_bytes([image[2], image[3]]), 0x0000);
        assert_eq!(u16::from_ne_bytes([image[4], image[5]]), 0x0000);
    }

    #[test]
    fn row_shorter_than_declared_width() {
        // When a pixel row has fewer characters than the declared width, the
        // decoder should silently leave the remaining pixels as zero
        // (transparent black), matching xfwm4's "row too short → continue"
        // behavior.  Here the row has 1 pixel but width=3.
        let data = b"/* XPM */
static char *test[] = {
\"3 1 1 1\",
\". c #FF0000\",
\".\",
};";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut image).unwrap();
        // 3 pixels * 8 bytes = 24 bytes.
        assert_eq!(image.len(), 24);
        // First pixel: red
        assert_eq!(u16::from_ne_bytes([image[0], image[1]]), 0xFFFF);
        assert_eq!(u16::from_ne_bytes([image[6], image[7]]), 0xFFFF);
        // Second and third pixels: zero-filled (transparent black)
        assert_eq!(&image[8..24], &[0u8; 16]);
    }

    #[test]
    fn more_rows_than_declared_height() {
        // Some XPM files declare a height smaller than the actual number of pixel rows.
        // The decoder should read exactly `height` rows and ignore extra rows before
        // the closing `};`.
        let data = b"/* XPM */
static char *test[] = {
\"2 2 1 1\",
\". c #FF0000\",
\"..\",
\"..\",
\"..\",
\"..\",
};";
        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut image).unwrap();
    }

    #[test]
    fn trailing_semicolon_required() {
        let data = b"/* XPM */
        static char *test[] = {
        \"1 1 1 1\",
        \"  c none\",
        \" \",
    };";
        let decoder = XpmDecoder::new(&data[..data.len() - 1], HashMap::new()).unwrap();
        let mut image = vec![0; decoder.total_bytes() as usize];
        assert!(decoder.read_image(&mut image).is_err());

        let decoder = XpmDecoder::new(&data[..], HashMap::new()).unwrap();
        let mut image = vec![0; decoder.total_bytes() as usize];
        assert!(decoder.read_image(&mut image).is_ok());
    }
}

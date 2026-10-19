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

//! Implements an LRU cache for `Icon` instances.  This is (so far) only used for window icons
//! displayed in SSDs.  The cache is global, and is `Rc`-cloned into each `WindowDecorations`
//! instance.  That way, if there are a bunch of windows (say, terminal windows) that share the
//! same icon, we'll only keep one cached icon, and we'll only load it once.
//!
//! The cache is keyed by (`Icon`, logical size, scale).  The capacity is dynamic, and grows and
//! shrinks with the number of SSD windows present (mapped or minimized), with two available slots
//! per window (to handle the case where a window is being dragged between outputs).
//!
//! On each successful lookup, a cache-global stamp is incremented and assigned to the cache entry.
//! Eviction is a Vec sort+truncate.

use std::{cell::RefCell, cmp::Ordering, rc::Rc};

use crate::util::{
    cairo_ext::CairoImageSurfaceExt,
    icon::{Argb32Pixels, Icon},
    icon_theme::IconTheme,
};

const MIN_CAPACITY: usize = 8;

#[derive(Clone, Default)]
pub struct IconCache {
    inner: Rc<RefCell<IconCacheInner>>,
}

struct IconCacheInner {
    icons: Vec<CachedIcon>,
    capacity: usize,
    next_stamp: u64,
}

struct CachedIcon {
    stamp: u64,
    icon: Icon,
    logical_size: u32,
    scale: f64,
    pixels: Argb32Pixels,
}

impl IconCache {
    pub fn set_capacity(&self, capacity: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.capacity = capacity.max(MIN_CAPACITY);
        inner.evict_if_needed();
    }

    pub fn capacity(&self) -> usize {
        self.inner.borrow().capacity
    }

    pub fn load<I: IconTheme>(&self, icon: &Icon, logical_size: u32, scale: f64, icon_theme: &I) -> Option<Argb32Pixels> {
        let mut inner = self.inner.borrow_mut();
        inner.lookup(icon, logical_size, scale).or_else(|| {
            match icon
                .load(logical_size, logical_size, scale, icon_theme)
                .and_then(|surface| surface.to_argb32_pixels())
            {
                Ok(pixels) => {
                    inner.insert(icon, logical_size, scale, &pixels);
                    Some(pixels)
                }
                Err(err) => {
                    tracing::warn!("Failed to load icon {icon:?}: {err}");
                    None
                }
            }
        })
    }

    pub fn clear(&self) {
        self.inner.borrow_mut().icons.clear();
    }
}

impl IconCacheInner {
    fn lookup(&mut self, icon: &Icon, logical_size: u32, scale: f64) -> Option<Argb32Pixels> {
        self.icons
            .iter_mut()
            .find(|cached| cached.icon == *icon && cached.logical_size == logical_size && cached.scale == scale)
            .map(|cached| {
                cached.stamp = self.next_stamp;
                self.next_stamp += 1;
                cached.pixels.clone()
            })
    }

    fn insert(&mut self, icon: &Icon, logical_size: u32, scale: f64, pixels: &Argb32Pixels) {
        let cached = CachedIcon {
            stamp: self.next_stamp,
            icon: icon.clone(),
            logical_size,
            scale,
            pixels: pixels.clone(),
        };
        self.next_stamp += 1;
        self.icons.push(cached);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        if self.icons.len() > self.capacity {
            self.icons.sort_by(|a, b| {
                if a.stamp > b.stamp {
                    Ordering::Less
                } else if a.stamp < b.stamp {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });
            self.icons.truncate(self.capacity);
        }
    }
}

impl Default for IconCacheInner {
    fn default() -> Self {
        Self {
            icons: Vec::with_capacity(MIN_CAPACITY),
            capacity: MIN_CAPACITY,
            next_stamp: 0,
        }
    }
}

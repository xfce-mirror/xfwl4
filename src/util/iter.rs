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

/// Zips `first` with `second`, ensuring all elements in `first` are iterated over.
///
/// Similar to .zip(), but instead of returning None when either iterator returns None, this
/// ensures that every element in `first` is iterated over, while every element in `second` is
/// wrapped in `Some` in the resulting iterator.  If `second` runs out before `first` does, there
/// will be `None` in the second slot of the tuple.
pub fn zip_all_first<I, J>(first: I, second: J) -> impl Iterator<Item = (I::Item, Option<J::Item>)>
where
    I: IntoIterator,
    J: IntoIterator,
{
    let mut second_iter = second.into_iter();
    first.into_iter().map(move |item| (item, second_iter.next()))
}

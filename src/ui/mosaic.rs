//! A container that packs tiles into rows, each tile as wide as its weight
//! allows, the way CSS `flex-wrap` with growing items would. A small gap
//! separates the rounded tiles.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::RefCell;

const GAP: i32 = 8;
const MIN_TILE_WIDTH: i32 = 170;
const TILE_HEIGHT: i32 = 112;
const MAX_PER_ROW: usize = 4;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Mosaic {
        pub tiles: RefCell<Vec<(gtk4::Widget, f64)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Mosaic {
        const NAME: &'static str = "MagnificoMosaic";
        type Type = super::Mosaic;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("mosaic");
        }
    }

    impl ObjectImpl for Mosaic {
        fn dispose(&self) {
            for (tile, _) in self.tiles.borrow_mut().drain(..) {
                tile.unparent();
            }
        }
    }

    impl WidgetImpl for Mosaic {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let count = self.tiles.borrow().len();
            if count == 0 {
                return (0, 0, -1, -1);
            }
            match orientation {
                gtk4::Orientation::Horizontal => (MIN_TILE_WIDTH, MIN_TILE_WIDTH * 4, -1, -1),
                _ => {
                    let width = if for_size > 0 {
                        for_size
                    } else {
                        MIN_TILE_WIDTH * 4
                    };
                    let rows = super::row_sizes(count, width).len() as i32;
                    let height = rows * TILE_HEIGHT + (rows - 1).max(0) * GAP;
                    (height, height, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let tiles = self.tiles.borrow();
            let weights: Vec<f64> = tiles.iter().map(|(_, w)| *w).collect();
            let mut index = 0;
            let mut y = 0;
            for row_len in super::row_sizes(tiles.len(), width) {
                let row = &weights[index..index + row_len];
                let widths = super::row_widths(row, width);
                let mut x = 0;
                for (offset, tile_width) in widths.into_iter().enumerate() {
                    let tile = &tiles[index + offset].0;
                    let transform = gtk4::gsk::Transform::new()
                        .translate(&gtk4::graphene::Point::new(x as f32, y as f32));
                    tile.allocate(tile_width, TILE_HEIGHT, -1, Some(transform));
                    x += tile_width + GAP;
                }
                index += row_len;
                y += TILE_HEIGHT + GAP;
            }
        }
    }
}

glib::wrapper! {
    pub struct Mosaic(ObjectSubclass<imp::Mosaic>)
        @extends gtk4::Widget;
}

impl Default for Mosaic {
    fn default() -> Self {
        Self::new()
    }
}

impl Mosaic {
    pub fn new() -> Self {
        let mosaic: Self = glib::Object::new();
        mosaic.set_overflow(gtk4::Overflow::Hidden);
        mosaic
    }

    /// Replace all tiles. `weight` decides how wide each tile grows.
    pub fn set_tiles(&self, tiles: Vec<(gtk4::Widget, f64)>) {
        for (tile, _) in self.imp().tiles.borrow_mut().drain(..) {
            tile.unparent();
        }
        for (tile, _) in &tiles {
            tile.set_parent(self);
        }
        self.imp().tiles.replace(tiles);
        self.queue_resize();
    }
}

/// How many tiles go in each row for this width: at most four, rows as
/// even as possible so no tile ends up alone and stretched.
fn row_sizes(count: usize, width: i32) -> Vec<usize> {
    let fits = (((width + GAP) / (MIN_TILE_WIDTH + GAP)).max(1)) as usize;
    let per_row = fits.min(MAX_PER_ROW);
    let rows = count.div_ceil(per_row);
    (0..rows)
        .map(|row| count / rows + usize::from(row < count % rows))
        .collect()
}

/// Share `width` between a row's tiles by weight, none below the minimum.
fn row_widths(weights: &[f64], width: i32) -> Vec<i32> {
    let n = weights.len();
    let available = (width - GAP * (n as i32 - 1)).max(0);
    let mut fixed = vec![false; n];
    loop {
        let free_width = available - fixed.iter().filter(|f| **f).count() as i32 * MIN_TILE_WIDTH;
        let free_weight: f64 = weights
            .iter()
            .zip(&fixed)
            .filter(|(_, f)| !**f)
            .map(|(w, _)| w.max(0.0001))
            .sum();
        let mut changed = false;
        let widths: Vec<i32> = weights
            .iter()
            .zip(fixed.iter_mut())
            .map(|(weight, fixed)| {
                if *fixed {
                    return MIN_TILE_WIDTH;
                }
                let share = (f64::from(free_width) * weight.max(0.0001) / free_weight) as i32;
                if share < MIN_TILE_WIDTH {
                    *fixed = true;
                    changed = true;
                }
                share
            })
            .collect();
        if !changed || fixed.iter().all(|f| *f) {
            let mut widths: Vec<i32> = widths
                .into_iter()
                .zip(&fixed)
                .map(|(w, f)| if *f { MIN_TILE_WIDTH } else { w })
                .collect();
            // Rounding leftovers go to the last tile so the row is flush.
            let used: i32 = widths.iter().sum();
            if let Some(last) = widths.last_mut() {
                *last += available - used;
            }
            return widths;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_fill_the_width_and_respect_the_minimum() {
        let widths = row_widths(&[100.0, 10.0, 1.0], 900);
        assert_eq!(widths.iter().sum::<i32>() + 2 * GAP, 900);
        assert!(widths.iter().all(|w| *w >= MIN_TILE_WIDTH));
        assert!(widths[0] > widths[1]);
        assert_eq!(row_sizes(7, 900), vec![4, 3]);
        assert_eq!(row_sizes(5, 900), vec![3, 2]);
    }
}

//! How many build plates the tiles still to print take: a shelf packing of
//! their footprints, which is close to what slicers manage for flat tiles.

/// A printer's build plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bed {
    pub width: f64,
    pub depth: f64,
    /// Space left between parts, in millimetres.
    pub gap: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packing {
    pub plates: usize,
    /// Parts larger than the plate even when turned.
    pub too_big: usize,
}

/// Packs parts (width, depth in mm) onto plates: each turned to lie long
/// side across if it fits, deepest first, in rows ("shelves") across each
/// plate.
pub fn pack(parts: &[(f64, f64)], bed: Bed) -> Packing {
    let mut fitted: Vec<(f64, f64)> = Vec::new();
    let mut too_big = 0;
    for &(a, b) in parts {
        let (long, short) = if a >= b { (a, b) } else { (b, a) };
        if long <= bed.width && short <= bed.depth {
            fitted.push((long, short));
        } else if long <= bed.depth && short <= bed.width {
            fitted.push((short, long));
        } else {
            too_big += 1;
        }
    }
    fitted.sort_by(|x, y| y.1.total_cmp(&x.1).then(y.0.total_cmp(&x.0)));

    // Each plate is a list of shelves: (depth used before it, its depth, width used)
    let mut plates: Vec<Vec<(f64, f64, f64)>> = Vec::new();
    'parts: for (w, d) in fitted {
        for plate in &mut plates {
            for shelf in plate.iter_mut() {
                if d <= shelf.1 && shelf.2 + bed.gap + w <= bed.width {
                    shelf.2 += bed.gap + w;
                    continue 'parts;
                }
            }
            let used = plate.last().map_or(0.0, |s| s.0 + s.1 + bed.gap);
            if used + d <= bed.depth {
                plate.push((used, d, w));
                continue 'parts;
            }
        }
        plates.push(vec![(0.0, d, w)]);
    }
    Packing { plates: plates.len(), too_big }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BED: Bed = Bed { width: 220.0, depth: 220.0, gap: 3.0 };

    #[test]
    fn small_tiles_share_a_plate() {
        // Sixteen 2" floors (50.8 mm) fit 4 x 4 on a 220 mm bed
        let floors = vec![(50.8, 50.8); 16];
        assert_eq!(pack(&floors, BED), Packing { plates: 1, too_big: 0 });
        let floors = vec![(50.8, 50.8); 17];
        assert_eq!(pack(&floors, BED).plates, 2);
    }

    #[test]
    fn parts_turn_to_fit_and_oversized_ones_are_counted() {
        let bed = Bed { width: 250.0, depth: 120.0, gap: 0.0 };
        assert_eq!(pack(&[(100.0, 240.0)], bed), Packing { plates: 1, too_big: 0 });
        assert_eq!(pack(&[(300.0, 50.0), (10.0, 10.0)], bed), Packing { plates: 1, too_big: 1 });
        assert_eq!(pack(&[], bed), Packing { plates: 0, too_big: 0 });
    }
}

//! Tile numbers for the printed plan, in the order a reader scans the map.

use mtp_core::tile::{Placed, PlacedId};

use crate::ExportInput;

/// Numbers the exported tiles 1..n in reading order on the printed map:
/// level by level, top row first (north is at the top), then left to right,
/// then bottom of a stack first. Ties keep document order, so the numbers
/// don't change between two exports of the same layout.
pub fn placements(input: &ExportInput) -> Vec<(PlacedId, u32)> {
    let mut tiles: Vec<&Placed> = input.tiles().collect();
    tiles.sort_by(|a, b| {
        let top = |t: &Placed| t.y + t.effective().1 as f32;
        a.level.cmp(&b.level).then(top(b).total_cmp(&top(a))).then(a.x.total_cmp(&b.x)).then(a.z.total_cmp(&b.z))
    });
    tiles.iter().zip(1..).map(|(t, n)| (t.id, n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    #[test]
    fn numbers_follow_reading_order() {
        let mut stacked = tile(5, "a/b.stl", 2, 2, 0.0, 0.0);
        stacked.z = 0.48;
        let s = state(
            20,
            20,
            vec![
                stacked,
                tile(1, "a/b.stl", 2, 2, 0.0, 0.0),
                tile(2, "a/b.stl", 2, 2, 4.0, 6.0),
                tile(3, "a/b.stl", 2, 2, 2.0, 6.0),
                // Bottom edge lower, but its top edge is the highest
                tile(4, "a/b.stl", 2, 6, 8.0, 3.0),
            ],
        );
        let input = ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None };
        let order: Vec<u64> = placements(&input).iter().map(|(id, _)| id.0).collect();
        assert_eq!(order, [4, 3, 2, 1, 5]);
        let numbers: Vec<u32> = placements(&input).iter().map(|(_, n)| *n).collect();
        assert_eq!(numbers, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn levels_come_in_order() {
        let mut upper = tile(1, "a/b.stl", 1, 1, 0.0, 10.0);
        upper.level = 1;
        let s = state(20, 20, vec![upper, tile(2, "a/b.stl", 1, 1, 0.0, 0.0)]);
        let input = ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None };
        assert_eq!(placements(&input)[0], (PlacedId(2), 1));
    }
}

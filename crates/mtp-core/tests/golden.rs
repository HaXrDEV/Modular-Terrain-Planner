//! Compares against what the Python version did (tests/golden/python.json,
//! written by tools/golden.py at the python-final tag).

use mtp_core::footprint::{Rotation, cells, effective_size, model_matrix};
use mtp_core::units::TileSize;
use serde_json::Value;

pub fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/python.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).expect("golden JSON")
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array().expect("array").iter().map(|x| x.as_f64().expect("number") as f32).collect()
}

#[test]
fn footprints_and_model_matrices_match_python() {
    let g = golden();
    let cases = g["footprints"].as_array().expect("footprints");
    assert!(cases.len() >= 36);
    for case in cases {
        let size = f32s(&case["size"]);
        let size = TileSize { w: size[0] as u32, h: size[1] as u32, gz: size[2] };
        let rotation = Rotation::from_degrees(case["rotation"].as_f64().expect("rotation")).expect("finite");
        let pos = f32s(&case["pos"]);
        let (ew, eh) = effective_size(size, rotation);
        assert_eq!(vec![ew as f32, eh as f32], f32s(&case["effective"]), "{case}");

        let expected: Vec<(i32, i32)> = case["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .map(|c| (c[0].as_i64().expect("x") as i32, c[1].as_i64().expect("y") as i32))
            .collect();
        assert_eq!(cells(pos[0], pos[1], ew, eh).collect::<Vec<_>>(), expected, "{case}");

        let m = model_matrix(pos[0], pos[1], pos[2], size, rotation);
        for (got, want) in m.iter().zip(f32s(&case["model_cols"])) {
            assert!((got - want).abs() < 1e-5, "{case}: {m:?}");
        }
    }
}

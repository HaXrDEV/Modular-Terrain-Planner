mod common;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use mtp_stl::load_many;

fn mixed() -> Vec<PathBuf> {
    ["good/floor_2x2_mm.stl", "bad/empty.stl", "good/ascii_corner.stl", "bad/truncated.stl", "bad/missing.stl"]
        .iter()
        .map(|p| common::fixture(p))
        .collect()
}

#[test]
fn one_result_per_file() {
    let paths = mixed();
    let results = Mutex::new(Vec::new());
    let reported = load_many(&paths, None, &AtomicBool::new(false), |i, r| {
        results.lock().expect("not poisoned").push((i, r.is_ok()));
    });
    let mut results = results.into_inner().expect("not poisoned");
    results.sort();
    assert_eq!(reported, paths.len());
    assert_eq!(results, [(0, true), (1, false), (2, true), (3, false), (4, false)]);
}

#[test]
fn cancelled_before_start_loads_nothing() {
    let calls = Mutex::new(0);
    let reported = load_many(&mixed(), None, &AtomicBool::new(true), |_, _| {
        *calls.lock().expect("not poisoned") += 1;
    });
    assert_eq!(reported, 0);
    assert_eq!(*calls.lock().expect("not poisoned"), 0);
}

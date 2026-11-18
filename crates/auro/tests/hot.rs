// SPDX-License-Identifier: MIT OR Apache-2.0

//! The boundary must return the compiled result when no dev host is attached: that is the
//! fallback every non-dev run and every `cargo test` relies on.

fn answer() -> u32 {
    42
}

fn increment(value: u32) -> u32 {
    value + 1
}

#[test]
fn hot_boundary_falls_back_to_the_compiled_function() {
    assert_eq!(auro::hot::hot(answer), 42);
    assert_eq!(auro::hot::hot_with(increment, 41), 42);
    assert_eq!(auro::hot::hot_with(|value: u32| value * 2, 21), 42);
}

#[test]
fn hot_boundary_is_repeatable() {
    let first = auro::hot::hot(answer);
    let second = auro::hot::hot(answer);
    assert_eq!(first, second);
}

#[test]
fn a_missing_patch_library_is_an_error_not_a_panic() {
    let table = subsecond::JumpTable {
        lib: std::env::temp_dir().join("auro-no-such-patch.so"),
        map: Default::default(),
        aslr_reference: 0,
        new_base_address: 0,
        ifunc_count: 0,
    };
    let applied = std::panic::catch_unwind(|| unsafe { subsecond::apply_patch(table) });
    assert!(
        matches!(applied, Ok(Err(_))),
        "a missing patch must return an error so the listener survives"
    );
}

#[test]
fn frame_boundary_can_report_after_validation() {
    let value = auro::hot::hot_frame(answer);
    assert_eq!(value, 42);
    auro::hot::rendered();
}

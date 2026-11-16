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

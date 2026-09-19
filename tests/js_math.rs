// `js_log` has to return what V8's `Math.log` returns, bit for bit: scores are
// compared exactly with JS MiniSearch's. The reference values come from Node
// (scripts/gen-log-fixture.mjs).
use minisearch_wasm::js_log;

#[test]
fn log_matches_math_log_bit_for_bit() {
    let fixture = include_str!("fixtures/math_log.txt");
    let mut checked = 0;
    for line in fixture.lines() {
        let (input, expected) = line.split_once(' ').expect("two columns");
        let input = f64::from_bits(u64::from_str_radix(input, 16).unwrap());
        let expected = f64::from_bits(u64::from_str_radix(expected, 16).unwrap());
        let actual = js_log(input);
        if expected.is_nan() {
            assert!(actual.is_nan(), "log({input:e})");
        } else {
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "log({input:e}): {actual:e} vs {expected:e}"
            );
        }
        checked += 1;
    }
    assert!(checked > 1500);
}

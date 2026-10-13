#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    let _spawned = tehuti::plugins::fuzz_stdout(bytes).is_ok();
});

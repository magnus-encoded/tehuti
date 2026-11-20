#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    tehuti::plugins::fuzz_stdout(bytes);
});

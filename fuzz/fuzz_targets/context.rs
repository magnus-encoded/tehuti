#![no_main]

libfuzzer_sys::fuzz_target!(|source: &str| {
    let _parsed = tehuti_protocol::parse_context(source);
});

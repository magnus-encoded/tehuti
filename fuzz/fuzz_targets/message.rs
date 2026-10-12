#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    let _result = tehuti_protocol::parse_message(bytes);
});

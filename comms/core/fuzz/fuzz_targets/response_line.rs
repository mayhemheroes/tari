#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    _ = tari_comms::tor::response_line(data);
    _ = tari_comms::tor::multi_key_value(data);
});

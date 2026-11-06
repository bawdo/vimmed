#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| editor_core::fuzz::markdown_incremental(data));

//! A sequence of legal moves, then one arbitrary token; see `qchess::fuzzing`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    qchess::fuzzing::run(data, 40);
});

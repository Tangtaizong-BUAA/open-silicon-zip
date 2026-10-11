#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Split input: first byte selects the code page label index, rest is the name bytes.
    let (sel, rest) = match data.split_first() {
        Some((s, r)) => (*s, r),
        None => return,
    };
    let labels = ["ibm437", "shift_jis", "gbk", "big5", "euc-kr", "windows-1252", "utf-8"];
    let label = labels[sel as usize % labels.len()];
    let _ = fz_core::codepage::decode(label, rest);
});

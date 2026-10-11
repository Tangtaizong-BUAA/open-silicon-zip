#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let dir = std::env::temp_dir().join(format!("fz-fuzz-tar-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("input.tar.gz");
    if std::fs::File::create(&path).and_then(|mut f| f.write_all(data)).is_err() {
        return;
    }
    let _ = fz_core::formats::list(&path, None, None, &fz_core::CancelToken::new());
});

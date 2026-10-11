#![no_main]
use fz_core::zip::{ReadOptions, ZipArchive};
use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let dir = std::env::temp_dir().join(format!("fz-fuzz-zip-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("input.zip");
    if std::fs::File::create(&path).and_then(|mut f| f.write_all(data)).is_err() {
        return;
    }
    if let Ok(arc) = ZipArchive::open(&path, ReadOptions::default()) {
        let _ = fz_core::zip::test_archive(&arc, None, &fz_core::CancelToken::new(), &fz_core::Progress::new());
    }
});

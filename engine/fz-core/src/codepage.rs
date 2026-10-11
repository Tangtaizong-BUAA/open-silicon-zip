//! Legacy code pages for ZIP entry names written without the UTF-8 flag (FR-801..805).
//!
//! Most pages are backed by `encoding_rs`. IBM437, CP850 and CP737 are not in
//! `encoding_rs`, so they use hand-written 128-entry tables for bytes 0x80..=0xFF.
//! The CP737 table is an approximation: its Greek letters follow the code page,
//! while the box-drawing and symbol ranges are borrowed from CP437.

use encoding_rs::Encoding;

/// One selectable legacy code page.
#[derive(Debug, Clone, Copy)]
pub struct CodePage {
    /// Stable identifier (an `encoding_rs` label, or `ibm437` / `cp850` / `cp737`).
    pub id: &'static str,
    /// Human readable name for the UI.
    pub label: &'static str,
    /// `encoding_rs` codec. Placeholder (windows-1252) for the manual pages, which do not use it.
    pub enc: &'static Encoding,
    /// High-half table (bytes 0x80..=0xFF) for pages not provided by `encoding_rs`.
    manual: Option<&'static [char; 128]>,
}

/// Code page catalog. `ibm437` comes before `cp850` so ties between them resolve to CP437.
pub const CATALOG: &[CodePage] = &[
    CodePage { id: "utf-8", label: "UTF-8", enc: encoding_rs::UTF_8, manual: None },
    CodePage { id: "ibm437", label: "IBM437 (CP437, US DOS)", enc: encoding_rs::WINDOWS_1252, manual: Some(&CP437_HIGH) },
    CodePage { id: "cp850", label: "CP850 (Western Europe, DOS)", enc: encoding_rs::WINDOWS_1252, manual: Some(&CP850_HIGH) },
    CodePage { id: "cp737", label: "Greek (CP737, DOS)", enc: encoding_rs::WINDOWS_1252, manual: Some(&CP737_HIGH) },
    CodePage { id: "gbk", label: "Chinese Simplified (CP936)", enc: encoding_rs::GBK, manual: None },
    CodePage { id: "big5", label: "Chinese Traditional (CP950)", enc: encoding_rs::BIG5, manual: None },
    CodePage { id: "shift_jis", label: "Japanese (CP932)", enc: encoding_rs::SHIFT_JIS, manual: None },
    CodePage { id: "euc-kr", label: "Korean (CP949)", enc: encoding_rs::EUC_KR, manual: None },
    CodePage { id: "windows-1250", label: "Central European (CP1250)", enc: encoding_rs::WINDOWS_1250, manual: None },
    CodePage { id: "windows-1251", label: "Cyrillic (CP1251)", enc: encoding_rs::WINDOWS_1251, manual: None },
    CodePage { id: "windows-1252", label: "Western European (CP1252)", enc: encoding_rs::WINDOWS_1252, manual: None },
    CodePage { id: "windows-1254", label: "Turkish (CP1254)", enc: encoding_rs::WINDOWS_1254, manual: None },
    CodePage { id: "windows-1255", label: "Hebrew (CP1255)", enc: encoding_rs::WINDOWS_1255, manual: None },
    CodePage { id: "windows-1256", label: "Arabic (CP1256)", enc: encoding_rs::WINDOWS_1256, manual: None },
    CodePage { id: "windows-1257", label: "Baltic (CP1257)", enc: encoding_rs::WINDOWS_1257, manual: None },
    CodePage { id: "windows-874", label: "Thai (CP874 / TIS-620)", enc: encoding_rs::WINDOWS_874, manual: None },
];

/// Copy of the catalog.
pub fn catalog() -> Vec<CodePage> {
    CATALOG.to_vec()
}

fn find(page_id: &str) -> Option<&'static CodePage> {
    CATALOG.iter().find(|c| c.id.eq_ignore_ascii_case(page_id))
}

/// Decodes raw name bytes. Returns the text and whether any byte was not
/// representable (replacement characters were inserted). An unknown page id
/// decodes as lossy UTF-8 and reports `true`.
pub fn decode(page_id: &str, bytes: &[u8]) -> (String, bool) {
    match find(page_id) {
        Some(cp) => match cp.manual {
            Some(high) => {
                let s: String = bytes
                    .iter()
                    .map(|&b| if b < 0x80 { b as char } else { high[(b - 0x80) as usize] })
                    .collect();
                (s, false)
            }
            None => {
                let (s, had) = cp.enc.decode_without_bom_handling(bytes);
                (s.into_owned(), had)
            }
        },
        None => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

/// Encodes text in the given page. Returns `None` for an unknown page or when a
/// character has no mapping in it.
pub fn encode(page_id: &str, s: &str) -> Option<Vec<u8>> {
    let cp = find(page_id)?;
    match cp.manual {
        Some(high) => s
            .chars()
            .map(|c| {
                if (c as u32) < 0x80 {
                    Some(c as u8)
                } else {
                    high.iter().position(|&h| h == c).map(|i| 0x80 + i as u8)
                }
            })
            .collect(),
        None => {
            if cp.id == "utf-8" {
                return Some(s.as_bytes().to_vec());
            }
            let (out, _, unmappable) = cp.enc.encode(s);
            if unmappable { None } else { Some(out.into_owned()) }
        }
    }
}

/// Picks the code page for a set of raw ZIP names.
///
/// 1. If `unix_utf8_ok` and at least 95% of the non-ASCII names are valid UTF-8,
///    returns `"utf-8"`.
/// 2. If every name is ASCII, returns `default_id` (or `"utf-8"` if it is unknown).
/// 3. Otherwise scores every catalog page on the decoded non-ASCII names and returns
///    the best. Ties go to `default_id` when it is among them, else to catalog order.
///
/// The scoring is a heuristic (no language model): it rewards characters that are
/// common in each script, penalises replacement, control and box-drawing characters,
/// penalises a word that mixes scripts, and penalises script/page mismatches.
pub fn auto_detect(names: &[&[u8]], default_id: &str, unix_utf8_ok: bool) -> &'static str {
    let default = find(default_id).map(|c| c.id).unwrap_or("utf-8");
    let non_ascii: Vec<&[u8]> = names.iter().copied().filter(|n| !n.is_ascii()).collect();
    if non_ascii.is_empty() {
        return default;
    }
    if unix_utf8_ok {
        let valid = non_ascii.iter().filter(|n| std::str::from_utf8(n).is_ok()).count();
        if valid * 100 >= non_ascii.len() * 95 {
            return "utf-8";
        }
    }

    let scores: Vec<(&'static str, i64)> = CATALOG
        .iter()
        .map(|cp| {
            let total = non_ascii
                .iter()
                .map(|n| {
                    let (s, _) = decode(cp.id, n);
                    score_text(cp.id, &s)
                })
                .sum();
            (cp.id, total)
        })
        .collect();

    let best = scores.iter().map(|&(_, s)| s).max().unwrap_or(0);
    let tied: Vec<&'static str> = scores.iter().filter(|&&(_, s)| s == best).map(|&(id, _)| id).collect();
    if tied.contains(&default) {
        default
    } else {
        tied.first().copied().unwrap_or("utf-8")
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

const L: u16 = 1 << 0; // Latin letters
const CY: u16 = 1 << 1;
const GR: u16 = 1 << 2;
const HAN: u16 = 1 << 3;
const KANA: u16 = 1 << 4;
const HANGUL: u16 = 1 << 5;
const TH: u16 = 1 << 6;
const HE: u16 = 1 << 7;
const AR: u16 = 1 << 8;
const OTHER: u16 = 1 << 9;

/// Frequent CJK characters (simplified, traditional and Japanese). Matching one is a strong signal.
const COMMON_CJK: &str = "中文件国的是了不在人有个这们来说时要会对为发还我他和也就上大出学生能地家作用好你工日本語繁體國學灣這個來說時發還會對為們體與於從將年月日";
/// Western accented letters.
const COMMON_WESTERN: &str = "àáâãäåæçèéêëìíîïñòóôõöøùúûüýÿßÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÑÒÓÔÕÖØÙÚÛÜÝ";
/// Central European and Turkish letters.
const COMMON_OTHER_LATIN: &str = "čćďđěĺľłńňőřśşšťůűźżžğıİąęĎČĆĚĹĽŇŘŚŞŠŤŮŰŹŻŽĞĄĘŁŃŐ";
/// Non-letter symbols that are normal in names.
const OK_PUNCT: &str = "«»“”‘’–—…€°±·£¥§¶©®™";

fn script_bit(c: char) -> u16 {
    if c.is_ascii_alphabetic() {
        return L;
    }
    match c as u32 {
        0xC0..=0xFF | 0x100..=0x24F => L,
        0x370..=0x3FF => GR,
        0x400..=0x4FF => CY,
        0x590..=0x5FF => HE,
        0x600..=0x6FF => AR,
        0xE00..=0xE7F => TH,
        0x3040..=0x30FF | 0xFF61..=0xFF9F => KANA,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => HAN,
        0x1100..=0x11FF | 0xAC00..=0xD7AF => HANGUL,
        _ => OTHER,
    }
}

fn is_cjk_page(page: &str) -> bool {
    matches!(page, "gbk" | "big5" | "shift_jis" | "euc-kr")
}

fn is_latin_page(page: &str) -> bool {
    matches!(page, "ibm437" | "cp850" | "windows-1250" | "windows-1252" | "windows-1254" | "windows-1257")
}

fn char_score(page: &str, c: char) -> i64 {
    if c.is_ascii() {
        return if c.is_control() { -4 } else { 0 };
    }
    if c == '\u{FFFD}' || c.is_control() {
        return -4;
    }
    let cp = c as u32;
    if (0x2500..=0x259F).contains(&cp) {
        // Box drawing and block elements: the usual sign of CJK bytes read as CP437.
        return -2;
    }
    if COMMON_CJK.contains(c) {
        return 3;
    }
    if !c.is_alphabetic() {
        return if OK_PUNCT.contains(c) { 0 } else { -1 };
    }
    let any = page == "utf-8";
    match script_bit(c) {
        L => {
            if is_latin_page(page) || any {
                if COMMON_WESTERN.contains(c) {
                    2
                } else if COMMON_OTHER_LATIN.contains(c) {
                    1
                } else {
                    0
                }
            } else {
                -1
            }
        }
        CY => if page == "windows-1251" || any { 2 } else { -2 },
        GR => if page == "cp737" || any { 2 } else { -2 },
        TH => if page == "windows-874" || any { 2 } else { -2 },
        HE => if page == "windows-1255" || any { 2 } else { -2 },
        AR => if page == "windows-1256" || any { 2 } else { -2 },
        HAN => if is_cjk_page(page) || any { 1 } else { -2 },
        KANA => {
            if page == "shift_jis" || any {
                // Half-width katakana is rare in names; full-width kana is a strong Japanese signal.
                if (0xFF61..=0xFF9F).contains(&cp) { 0 } else { 2 }
            } else {
                -2
            }
        }
        HANGUL => if page == "euc-kr" || any { 2 } else { -2 },
        _ => -1,
    }
}

/// Penalises words that mix scripts without CJK (e.g. Latin letters with a Cyrillic one).
fn word_mix_penalty(s: &str) -> i64 {
    let mut penalty = 0;
    let mut mask: u16 = 0;
    let flush = |mask: &mut u16, penalty: &mut i64| {
        let mixed = mask.count_ones() >= 2 && (*mask & (HAN | KANA | HANGUL)) == 0;
        if mixed {
            *penalty -= 3;
        }
        *mask = 0;
    };
    for c in s.chars() {
        if c.is_alphabetic() {
            mask |= script_bit(c);
        } else {
            flush(&mut mask, &mut penalty);
        }
    }
    flush(&mut mask, &mut penalty);
    penalty
}

/// Scores decoded text. Characters from double-byte pages count double (they cost two
/// input bytes), so a CJK reading of CJK bytes is not outscored by a single-byte reading.
fn score_text(page: &str, s: &str) -> i64 {
    let multi = is_cjk_page(page);
    let weighted: i64 = s
        .chars()
        .map(|c| {
            let double = multi && !c.is_ascii() && !(0xFF61..=0xFF9F).contains(&(c as u32));
            let w = if double { 2 } else { 1 };
            w * char_score(page, c)
        })
        .sum();
    weighted + word_mix_penalty(s)
}

// ---------------------------------------------------------------------------
// Manual high-half tables (bytes 0x80..=0xFF)
// ---------------------------------------------------------------------------

const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}', //
];

const CP850_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', 'ø', '£', 'Ø', '×', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '®', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', 'Á', 'Â', 'À', '©', '╣', '║', '╗', '╝', '¢', '¥', '┐', //
    '└', '┴', '┬', '├', '─', '┼', 'ã', 'Ã', '╚', '╔', '╩', '╦', '╠', '═', '╬', '¤', //
    'ð', 'Ð', 'Ê', 'Ë', 'È', 'ı', 'Í', 'Î', 'Ï', '┘', '┌', '█', '▄', '¦', 'Ì', '▀', //
    'Ó', 'ß', 'Ô', 'Ò', 'õ', 'Õ', 'µ', 'þ', 'Þ', 'Ú', 'Û', 'Ù', 'ý', 'Ý', '¯', '´', //
    '\u{ad}', '±', '‗', '¾', '¶', '§', '÷', '¸', '°', '¨', '·', '¹', '³', '²', '■', '\u{a0}', //
];

const CP737_HIGH: [char; 128] = [
    'Α', 'Β', 'Γ', 'Δ', 'Ε', 'Ζ', 'Η', 'Θ', 'Ι', 'Κ', 'Λ', 'Μ', 'Ν', 'Ξ', 'Ο', 'Π', //
    'Ρ', 'Σ', 'Τ', 'Υ', 'Φ', 'Χ', 'Ψ', 'Ω', 'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', //
    'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ', 'ς', 'τ', 'υ', 'φ', 'χ', 'ψ', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'ά', 'έ', 'ή', 'ί', 'ό', 'ύ', 'ώ', 'Ά', 'Έ', 'Ή', 'Ί', 'Ό', 'Ύ', 'Ώ', 'Ϊ', 'Ϋ', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}', //
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes encoded with encoding_rs, independent of the code under test.
    fn bytes(enc: &'static Encoding, s: &str) -> Vec<u8> {
        enc.encode(s).0.into_owned()
    }

    #[test]
    fn catalog_has_all_pages_and_unique_ids() {
        let cat = catalog();
        assert!(cat.len() >= 16);
        let mut ids: Vec<_> = cat.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), cat.len());
        assert_eq!(cat[0].id, "utf-8");
    }

    #[test]
    fn manual_pages_roundtrip_and_known_values() {
        assert_eq!(decode("ibm437", &[0x82]).0, "é");
        assert_eq!(decode("ibm437", &[0xB0, 0xDB]).0, "░█");
        assert_eq!(decode("cp850", &[0xD5]).0, "ı");
        assert_eq!(decode("cp737", &[0x80, 0xA0]).0, "Αι");
        for page in ["ibm437", "cp850", "cp737"] {
            // Every high-half char must round-trip through encode/decode.
            let table: &[char; 128] = find(page).unwrap().manual.unwrap();
            let s: String = table.iter().collect();
            let enc = encode(page, &s).unwrap_or_else(|| panic!("{page} encode failed"));
            assert_eq!(enc, (0x80u8..=0xFF).collect::<Vec<u8>>(), "{page}");
            assert_eq!(decode(page, &enc).0, s, "{page}");
        }
        assert_eq!(encode("ibm437", "café").unwrap(), b"caf\x82".to_vec());
        assert!(encode("ibm437", "中").is_none());
    }

    #[test]
    fn encoding_rs_pages_decode_and_encode() {
        let (s, had) = decode("gbk", &bytes(encoding_rs::GBK, "中文件.txt"));
        assert_eq!(s, "中文件.txt");
        assert!(!had);
        assert_eq!(encode("gbk", "中文件.txt").unwrap(), bytes(encoding_rs::GBK, "中文件.txt"));
        assert!(encode("windows-1252", "中").is_none());
        let (s, had) = decode("shift_jis", &[0xFF, 0xFF]);
        assert!(had);
        assert!(s.contains('\u{FFFD}'));
    }

    #[test]
    fn unknown_page_is_lossy_utf8() {
        let (s, had) = decode("nope", "é".as_bytes());
        assert_eq!(s, "é");
        assert!(had);
        assert!(encode("nope", "x").is_none());
    }

    #[test]
    fn detect_utf8_when_unix_flag_set() {
        let u = "日本語.txt".as_bytes();
        assert_eq!(auto_detect(&[u, b"a.txt"], "ibm437", true), "utf-8");
        // Not enough valid UTF-8 for the flag: falls through to scoring.
        let g = bytes(encoding_rs::GBK, "中文件.txt");
        assert_eq!(auto_detect(&[&g], "ibm437", true), "gbk");
    }

    #[test]
    fn detect_ascii_returns_default() {
        assert_eq!(auto_detect(&[b"a.txt", b"b.txt"], "windows-1252", false), "windows-1252");
        assert_eq!(auto_detect(&[], "cp850", false), "cp850");
        assert_eq!(auto_detect(&[b"a"], "bogus", false), "utf-8");
    }

    #[test]
    fn detect_gbk() {
        let g = bytes(encoding_rs::GBK, "中文件.txt");
        assert_eq!(auto_detect(&[&g], "windows-1252", false), "gbk");
        assert_eq!(auto_detect(&[&g], "ibm437", false), "gbk");
    }

    #[test]
    fn detect_shift_jis() {
        let s = bytes(encoding_rs::SHIFT_JIS, "日本語.txt");
        assert_eq!(auto_detect(&[&s], "windows-1252", false), "shift_jis");
    }

    #[test]
    fn detect_big5() {
        let b = bytes(encoding_rs::BIG5, "繁體中文.txt");
        assert_eq!(auto_detect(&[&b], "windows-1252", false), "big5");
    }

    #[test]
    fn detect_euc_kr() {
        let k = bytes(encoding_rs::EUC_KR, "한국어.txt");
        assert_eq!(auto_detect(&[&k], "windows-1252", false), "euc-kr");
    }

    #[test]
    fn detect_cyrillic() {
        let c = bytes(encoding_rs::WINDOWS_1251, "Привет.txt");
        assert_eq!(auto_detect(&[&c], "windows-1252", false), "windows-1251");
        assert_eq!(auto_detect(&[&c], "ibm437", false), "windows-1251");
    }

    #[test]
    fn detect_cp437_cafe() {
        let c = b"caf\x82.txt";
        let got = auto_detect(&[c], "ibm437", false);
        assert_eq!(got, "ibm437");
        let got = auto_detect(&[c], "windows-1252", false);
        assert!(got == "ibm437" || got == "windows-1252", "{got}");
    }

    #[test]
    fn detect_cp1252_naive() {
        let n = bytes(encoding_rs::WINDOWS_1252, "naïve.txt");
        assert_eq!(auto_detect(&[&n], "windows-1252", false), "windows-1252");
    }
}

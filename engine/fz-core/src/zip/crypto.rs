//! ZIP encryption: legacy ZipCrypto and WinZip AES (AE-2).
use crate::error::{Error, ErrorKind, Result};
use aes::cipher::{BlockCipherEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha1::Sha1;

pub fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut v = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut v)).expect("urandom");
    v
}

// ---------- ZipCrypto ----------
pub struct ZipCrypto { k: [u32; 3] }
fn crc_byte(c: u32, b: u8) -> u32 {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 { let mut c = i; for _ in 0..8 { c = if c & 1 == 1 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 }; } t[i as usize] = c; }
        t
    });
    t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8)
}
impl ZipCrypto {
    pub fn new(pw: &[u8]) -> Self {
        let mut z = ZipCrypto { k: [0x12345678, 0x23456789, 0x34567890] };
        for &b in pw { z.update(b); }
        z
    }
    fn update(&mut self, b: u8) {
        self.k[0] = crc_byte(self.k[0], b);
        self.k[1] = self.k[1].wrapping_add(self.k[0] & 0xff).wrapping_mul(134775813).wrapping_add(1);
        self.k[2] = crc_byte(self.k[2], (self.k[1] >> 24) as u8);
    }
    fn stream(&self) -> u8 { let t = (self.k[2] | 2) as u16; ((t.wrapping_mul(t ^ 1)) >> 8) as u8 }
    pub fn encrypt(&mut self, buf: &mut [u8]) { for b in buf { let p = *b; *b ^= self.stream(); self.update(p); } }
    pub fn decrypt(&mut self, buf: &mut [u8]) { for b in buf { *b ^= self.stream(); self.update(*b); } }
    /// 12-byte encryption header; last byte is `check`.
    pub fn header(&mut self, check: u8) -> [u8; 12] {
        let mut h = [0u8; 12];
        h[..11].copy_from_slice(&random_bytes(11));
        h[11] = check;
        self.encrypt(&mut h);
        h
    }
}

// ---------- WinZip AES (AE-2) ----------
pub fn aes_salt_len(strength: u8) -> usize { match strength { 1 => 8, 2 => 12, _ => 16 } }
pub fn aes_key_len(strength: u8) -> usize { match strength { 1 => 16, 2 => 24, _ => 32 } }

enum Cipher { A128(aes::Aes128), A192(aes::Aes192), A256(aes::Aes256) }
impl Cipher {
    fn new(key: &[u8]) -> Self {
        match key.len() {
            16 => Cipher::A128(aes::Aes128::new_from_slice(key).unwrap()),
            24 => Cipher::A192(aes::Aes192::new_from_slice(key).unwrap()),
            _ => Cipher::A256(aes::Aes256::new_from_slice(key).unwrap()),
        }
    }
    fn enc(&self, blk: &mut [u8; 16]) {
        let b: &mut aes::Block = blk.into();
        match self { Cipher::A128(c) => c.encrypt_block(b), Cipher::A192(c) => c.encrypt_block(b), Cipher::A256(c) => c.encrypt_block(b) }
    }
}

pub struct AesCtx { cipher: Cipher, mac: Hmac<Sha1>, counter: u128, ks: [u8; 16], ks_pos: usize, pub salt: Vec<u8>, pub verifier: [u8; 2] }

fn derive(pw: &[u8], salt: &[u8], strength: u8) -> (Vec<u8>, Vec<u8>, [u8; 2]) {
    let kl = aes_key_len(strength);
    let mut out = vec![0u8; 2 * kl + 2];
    pbkdf2::pbkdf2_hmac::<Sha1>(pw, salt, 1000, &mut out);
    (out[..kl].to_vec(), out[kl..2 * kl].to_vec(), [out[2 * kl], out[2 * kl + 1]])
}

impl AesCtx {
    pub fn new_encrypt(pw: &[u8], strength: u8) -> Self {
        let salt = random_bytes(aes_salt_len(strength));
        Self::with_salt(pw, strength, salt)
    }
    pub fn with_salt(pw: &[u8], strength: u8, salt: Vec<u8>) -> Self {
        let (ek, mk, verifier) = derive(pw, &salt, strength);
        AesCtx { cipher: Cipher::new(&ek), mac: <Hmac<Sha1> as KeyInit>::new_from_slice(&mk).unwrap(), counter: 1, ks: [0; 16], ks_pos: 16, salt, verifier }
    }
    fn xor_stream(&mut self, buf: &mut [u8]) {
        // keystream position persists across calls so arbitrary chunk sizes stay aligned
        for b in buf.iter_mut() {
            if self.ks_pos == 16 {
                self.ks = self.counter.to_le_bytes();
                self.counter += 1;
                self.cipher.enc(&mut self.ks);
                self.ks_pos = 0;
            }
            *b ^= self.ks[self.ks_pos];
            self.ks_pos += 1;
        }
    }
    pub fn encrypt(&mut self, buf: &mut [u8]) { self.xor_stream(buf); self.mac.update(buf); }
    pub fn decrypt(&mut self, buf: &mut [u8]) { self.mac.update(buf); self.xor_stream(buf); }
    /// 10-byte authentication code.
    pub fn finish(self) -> [u8; 10] {
        let t = self.mac.finalize().into_bytes();
        let mut o = [0u8; 10];
        o.copy_from_slice(&t[..10]);
        o
    }
}

pub fn check_aes_password(pw: &[u8], strength: u8, salt: &[u8], verifier: [u8; 2]) -> Result<AesCtx> {
    let c = AesCtx::with_salt(pw, strength, salt.to_vec());
    if c.verifier != verifier { return Err(Error::new(ErrorKind::WrongPassword, "AES password verifier mismatch")); }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zipcrypto_roundtrip() {
        let mut e = ZipCrypto::new(b"pw");
        let h = e.header(0x42);
        let mut data = b"hello world".to_vec();
        e.encrypt(&mut data);
        let mut d = ZipCrypto::new(b"pw");
        let mut hh = h;
        d.decrypt(&mut hh);
        assert_eq!(hh[11], 0x42);
        d.decrypt(&mut data);
        assert_eq!(data, b"hello world");
    }
    #[test]
    fn aes_chunk_independence() {
        let data: Vec<u8> = (0..5000u32).map(|x| (x * 7) as u8).collect();
        let salt = vec![1u8; 16];
        let mut a = data.clone();
        AesCtx::with_salt(b"p", 3, salt.clone()).encrypt(&mut a);
        let mut b = data.clone();
        let mut c = AesCtx::with_salt(b"p", 3, salt);
        for ch in b.chunks_mut(37) { c.encrypt(ch); }
        assert_eq!(a, b);
    }
    #[test]
    fn aes_roundtrip() {
        for s in 1..=3u8 {
            let mut e = AesCtx::new_encrypt(b"secret", s);
            let mut data: Vec<u8> = (0..1000u32).map(|x| x as u8).collect();
            let orig = data.clone();
            e.encrypt(&mut data);
            assert_ne!(data, orig);
            let (salt, ver) = (e.salt.clone(), e.verifier);
            let mac = e.finish();
            let mut d = check_aes_password(b"secret", s, &salt, ver).unwrap();
            d.decrypt(&mut data);
            assert_eq!(data, orig);
            assert_eq!(d.finish(), mac);
            assert!(check_aes_password(b"nope", s, &salt, ver).is_err());
        }
    }
}

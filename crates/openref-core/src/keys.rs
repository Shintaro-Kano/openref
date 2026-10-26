/// Zotero に倣った 8 桁 key。紛らわしい文字 (0/O/1/I/L) を除いた英数。
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

pub fn new_key() -> String {
    (0..8)
        .map(|_| ALPHABET[fastrand::usize(..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shape() {
        for _ in 0..100 {
            let k = new_key();
            assert_eq!(k.len(), 8);
            assert!(k.bytes().all(|b| ALPHABET.contains(&b)));
        }
    }
}

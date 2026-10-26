//! wg-hash: SHA-256 digests, in-crate so that `Core` crates need no external dependency.
//!
//! The digest matches Python's `hashlib.sha256(...).hexdigest()` byte for byte. It is used for
//! the task content hash (`wg-tasks`) and the tracker snapshot digest (`wg-trackers`).

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INITIAL_STATE: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// The SHA-256 digest of `data`, as 64 lowercase hex digits.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    // Padding: 0x80, zeros until the length is 56 mod 64, then the 64-bit big-endian bit length.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let zeros = (119 - data.len() % 64) % 64;
    let message: Vec<u8> = data
        .iter()
        .copied()
        .chain(std::iter::once(0x80))
        .chain(std::iter::repeat_n(0, zeros))
        .chain(bit_len.to_be_bytes())
        .collect();

    message
        .chunks_exact(64)
        .fold(INITIAL_STATE, compress)
        .iter()
        .map(|word| format!("{word:08x}"))
        .collect()
}

fn compress(state: [u32; 8], block: &[u8]) -> [u32; 8] {
    let w = schedule(block);
    let rounds = (0..64).fold(state, |s, i| round(s, K[i], w[i]));
    std::array::from_fn(|i| state[i].wrapping_add(rounds[i]))
}

/// The 64-word message schedule for one 512-bit block.
fn schedule(block: &[u8]) -> [u32; 64] {
    let first_sixteen: [u32; 64] = std::array::from_fn(|i| {
        if i < 16 {
            u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ])
        } else {
            0
        }
    });
    (16..64).fold(first_sixteen, |w, i| {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        let next = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
        std::array::from_fn(|j| if j == i { next } else { w[j] })
    })
}

/// One compression round over the working variables `[a, b, c, d, e, f, g, h]`.
fn round(vars: [u32; 8], k: u32, w: u32) -> [u32; 8] {
    let [a, b, c, d, e, f, g, h] = vars;
    let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
    let choose = (e & f) ^ (!e & g);
    let t1 = h
        .wrapping_add(s1)
        .wrapping_add(choose)
        .wrapping_add(k)
        .wrapping_add(w);
    let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
    let majority = (a & b) ^ (a & c) ^ (b & c);
    let t2 = s0.wrapping_add(majority);
    [t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_matches_a_message_longer_than_one_block() {
        // Python: hashlib.sha256(b"a" * 1000).hexdigest()
        let data = vec![b'a'; 1000];
        assert_eq!(
            sha256_hex(&data),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }
}

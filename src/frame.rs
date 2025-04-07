use crate::Opcode;

// 2 byte header + 4 byte masking key.
const CONTROL_HEADER_LEN: usize = 6;
const MASK_BIT: u8 = 0x80;

#[derive(Clone, Copy, Debug)]
pub enum Message {
    Binary,
    Text,
}

impl Message {
    #[must_use]
    pub fn is_binary(self) -> bool {
        matches!(self, Self::Binary)
    }

    #[must_use]
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub fin: bool,
    pub opcode: Opcode,
}

impl Frame {
    pub const CONTROL_HEADER_LEN: usize = CONTROL_HEADER_LEN;

    /// Header space needs to be pre-allocated for the slice!
    #[inline]
    pub fn encode_control_slice(self, data: &mut [u8], mask: [u8; 4]) {
        let data_len = data.len() - CONTROL_HEADER_LEN;

        // SAFE IMPL
        // // Assumes data is contained in the 0..data.len() - 6 part.
        // for i in (Self::CONTROL_HEADER_LEN..data.len()).rev() {
        //     let j = i - Self::CONTROL_HEADER_LEN;
        //     data[i] = data[j] ^ mask[j & 3];
        // }

        // data[0] = ((self.fin as u8) << 7) | self.opcode as u8;
        // data[1] = Self::MASK_BIT | data_len as u8;

        // data[2..6].copy_from_slice(&mask);

        // UNSAFE IMPL
        unsafe {
            let dst = data.as_mut_ptr();
            mask_data::<CONTROL_HEADER_LEN>(dst, data_len, mask);
            dst.write(((self.fin as u8) << 7) | self.opcode as u8);
            dst.add(1).write(MASK_BIT | data_len as u8);
            std::ptr::copy_nonoverlapping(mask.as_ptr(), data.as_mut_ptr().add(2), mask.len());
        }
    }

    #[inline]
    pub fn encode_vec(self, data: &mut Vec<u8>, mask: [u8; 4]) {
        let data_len = data.len();
        let header_len = match data_len {
            ..126 => 6,
            126..65536 => 8,
            _ => 14,
        };

        data.resize(data_len + header_len, 0);

        // SAFE IMPL
        // for i in (header_len..data.len()).rev() {
        //     let j = i - header_len;
        //     data[i] = data[j] ^ mask[j & 3];
        // }

        // data[0] = ((self.fin as u8) << 7) | self.opcode as u8;

        // match header_len {
        //     6 => {
        //         data[1] = Self::MASK_BIT | data_len as u8;
        //         data[2..6].copy_from_slice(&mask);
        //     }
        //     8 => {
        //         let data_len_bytes = (data_len as u16).to_be_bytes();
        //         data[1] = Self::MASK_BIT | 126;
        //         data[2..4].copy_from_slice(&data_len_bytes);
        //         data[4..8].copy_from_slice(&mask);
        //     }
        //     14 => {
        //         let data_len_bytes = (data_len as u64).to_be_bytes();
        //         data[1] = Self::MASK_BIT | 127;
        //         data[2..10].copy_from_slice(&data_len_bytes);
        //         data[10..14].copy_from_slice(&mask);
        //     }
        //     _ => unreachable!(),
        // }

        // UNSAFE IMPL
        unsafe {
            let dst = data.as_mut_ptr();
            match header_len {
                6 => {
                    mask_data::<6>(dst, data_len, mask);
                    dst.write(((self.fin as u8) << 7) | self.opcode as u8);
                    dst.add(1).write(MASK_BIT | data_len as u8);
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(2), mask.len());
                }
                8 => {
                    mask_data::<8>(dst, data_len, mask);
                    dst.write(((self.fin as u8) << 7) | self.opcode as u8);
                    let data_len_bytes = (data_len as u16).to_be_bytes();
                    dst.add(1).write(MASK_BIT | 126);
                    std::ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dst.add(2),
                        data_len_bytes.len(),
                    );
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(4), mask.len());
                }
                14 => {
                    mask_data::<14>(dst, data_len, mask);
                    dst.write(((self.fin as u8) << 7) | self.opcode as u8);
                    let data_len_bytes = (data_len as u64).to_be_bytes();
                    dst.add(1).write(MASK_BIT | 127);
                    std::ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dst.add(2),
                        data_len_bytes.len(),
                    );
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(10), mask.len());
                }
                _ => unreachable!(),
            }
        }
    }

    #[inline]
    #[must_use]
    pub fn validate_utf8(data: &[u8]) -> Option<&str> {
        simdutf8::basic::from_utf8(data).ok()
    }
}

unsafe fn mask_data<const HEADER_LEN: usize>(dst: *mut u8, len: usize, mask: [u8; 4]) {
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            if len >= 16 && is_x86_feature_detected!("ssse3") {
                return mask_simd_x86::<HEADER_LEN>(dst, len, mask);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if len >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
                return mask_simd_aarch::<HEADER_LEN>(dst, len, mask);
            }
        }
        mask_scalar::<HEADER_LEN>(dst, len, mask);
    }
}

#[inline]
unsafe fn mask_scalar<const HEADER_LEN: usize>(dst: *mut u8, len: usize, mask: [u8; 4]) {
    for i in (0..len).rev() {
        let j = i + HEADER_LEN;
        unsafe {
            dst.add(j)
                .write(dst.add(i).read() ^ mask.get_unchecked(i & 3));
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
#[inline]
unsafe fn mask_simd_x86<const HEADER_LEN: usize>(dst: *mut u8, len: usize, mask: [u8; 4]) {
    use std::arch::x86_64::{
        __m128i, _mm_loadu_si128, _mm_set1_epi32, _mm_storeu_si128, _mm_xor_si128,
    };

    let chunks = len / 16;
    unsafe {
        // Handle remaining bytes first individually.
        for i in (chunks * 16..len).rev() {
            let j = i + HEADER_LEN;
            dst.add(j)
                .write(dst.add(i).read() ^ mask.get_unchecked(i & 3));
        }

        // Then handle full chunks with SIMD.
        let mask_value = std::mem::transmute::<[u8; 4], i32>(mask);
        let mask_x4 = _mm_set1_epi32(mask_value);
        for i in (0..chunks).rev() {
            let i = i * 16;
            let j = i + HEADER_LEN;
            let src = _mm_loadu_si128(dst.add(i) as *const __m128i);
            let masked = _mm_xor_si128(src, mask_x4);
            _mm_storeu_si128(dst.add(j).cast::<__m128i>(), masked);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn mask_simd_aarch<const HEADER_LEN: usize>(dst: *mut u8, len: usize, mask: [u8; 4]) {
    use std::arch::aarch64::{uint8x16_t, uint32x4_t, vdupq_n_u32, veorq_u8, vld1q_u8, vst1q_u8};

    let chunks = len / 16;
    unsafe {
        // Handle remaining bytes first individually.
        for i in (chunks * 16..len).rev() {
            let j = i + HEADER_LEN;
            dst.add(j)
                .write(dst.add(i).read() ^ mask.get_unchecked(i & 3));
        }

        // Then handle full chunks with SIMD.
        let mask_value = std::mem::transmute::<[u8; 4], u32>(mask);
        let mask_x4 = std::mem::transmute::<uint32x4_t, uint8x16_t>(vdupq_n_u32(mask_value));
        for i in (0..chunks).rev() {
            let i = i * 16;
            let j = i + HEADER_LEN;
            let src = vld1q_u8(dst.add(i).cast_const());
            let masked = veorq_u8(src, mask_x4);
            vst1q_u8(dst.add(j), masked);
        }
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(
        // ""
        vec![] =>
        vec![130, 128, 10, 241, 34, 51];
        "0"
    )]
    #[test_case(
        // "hell"
        vec![0x68, 0x65, 0x6C, 0x6C] =>
        vec![130, 132, 10, 241, 34, 51, 98, 148, 78, 95];
        "4"
    )]
    #[test_case(
        // "hello"
        vec![0x68, 0x65, 0x6C, 0x6C, 0x6F] =>
        vec![130, 133, 10, 241, 34, 51, 98, 148, 78, 95, 101];
        "5"
    )]
    #[test_case(
        // "hello world"
        vec![104, 101, 108, 108, 111, 32, 119, 111, 114, 108, 100] =>
        vec![130, 139, 10, 241, 34, 51, 98, 148, 78, 95, 101, 209, 85, 92, 120, 157, 70];
        "11"
    )]
    #[test_case(
        // "lorem ipsum dolo"
        vec![108, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111] =>
        vec![
            130, 144, 10, 241, 34, 51, 102, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92
        ];
        "16"
    )]
    #[test_case(
        // "lorem ipsum dolor"
        vec![108, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111, 114] =>
        vec![
            130, 145, 10, 241, 34, 51, 102, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92, 120
        ];
        "17"
    )]
    // "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
    // tempor incididunt ut labore et dolore magna aliqua. U"
    #[test_case(
        vec![
            76, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111, 114, 32,
            115, 105, 116, 32, 97, 109, 101, 116, 44, 32, 99, 111, 110, 115, 101, 99, 116, 101, 116,
            117, 114, 32, 97, 100, 105, 112, 105, 115, 99, 105, 110, 103, 32, 101, 108, 105, 116,
            44, 32, 115, 101, 100, 32, 100, 111, 32, 101, 105, 117, 115, 109, 111, 100, 32, 116,
            101, 109, 112, 111, 114, 32, 105, 110, 99, 105, 100, 105, 100, 117, 110, 116, 32, 117,
            116, 32, 108, 97, 98, 111, 114, 101, 32, 101, 116, 32, 100, 111, 108, 111, 114, 101, 32,
            109, 97, 103, 110, 97, 32, 97, 108, 105, 113, 117, 97, 46, 32, 85
        ] =>
        vec![
            130, 253, 10, 241, 34, 51, 70, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92, 120, 209, 81, 90, 126, 209, 67, 94, 111, 133, 14, 19, 105, 158, 76, 64,
            111, 146, 86, 86, 126, 132, 80, 19, 107, 149, 75, 67, 99, 130, 65, 90, 100, 150, 2, 86,
            102, 152, 86, 31, 42, 130, 71, 87, 42, 149, 77, 19, 111, 152, 87, 64, 103, 158, 70, 19,
            126, 148, 79, 67, 101, 131, 2, 90, 100, 146, 75, 87, 99, 149, 87, 93, 126, 209, 87, 71,
            42, 157, 67, 81, 101, 131, 71, 19, 111, 133, 2, 87, 101, 157, 77, 65, 111, 209, 79, 82,
            109, 159, 67, 19, 107, 157, 75, 66, 127, 144, 12, 19, 95
        ];
        "125"
    )]
    fn test_encode_control_slice(mut input: Vec<u8>) -> Vec<u8> {
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
        };
        let mask = [0x0a, 0xf1, 0x22, 0x33];

        input.resize(input.len() + Frame::CONTROL_HEADER_LEN, 0);
        frame.encode_control_slice(&mut input, mask);

        input
    }

    // "hello"
    #[test_case(
        vec![0x68, 0x65, 0x6C, 0x6C, 0x6F] =>
        vec![130, 133, 10, 241, 34, 51, 98, 148, 78, 95, 101];
        "5"
    )]
    #[test_case(
        // "lorem ipsum dolo"
        vec![108, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111] =>
        vec![
            130, 144, 10, 241, 34, 51, 102, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92
        ];
        "16"
    )]
    #[test_case(
        // "lorem ipsum dolor"
        vec![108, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111, 114] =>
        vec![
            130, 145, 10, 241, 34, 51, 102, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92, 120
        ];
        "17"
    )]
    // "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
    // tempor incididunt ut labore et dolore magna aliqua. U"
    #[test_case(
        vec![
            76, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111, 114, 32,
            115, 105, 116, 32, 97, 109, 101, 116, 44, 32, 99, 111, 110, 115, 101, 99, 116, 101, 116,
            117, 114, 32, 97, 100, 105, 112, 105, 115, 99, 105, 110, 103, 32, 101, 108, 105, 116,
            44, 32, 115, 101, 100, 32, 100, 111, 32, 101, 105, 117, 115, 109, 111, 100, 32, 116,
            101, 109, 112, 111, 114, 32, 105, 110, 99, 105, 100, 105, 100, 117, 110, 116, 32, 117,
            116, 32, 108, 97, 98, 111, 114, 101, 32, 101, 116, 32, 100, 111, 108, 111, 114, 101, 32,
            109, 97, 103, 110, 97, 32, 97, 108, 105, 113, 117, 97, 46, 32, 85
        ] =>
        vec![
            130, 253, 10, 241, 34, 51, 70, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19, 110,
            158, 78, 92, 120, 209, 81, 90, 126, 209, 67, 94, 111, 133, 14, 19, 105, 158, 76, 64,
            111, 146, 86, 86, 126, 132, 80, 19, 107, 149, 75, 67, 99, 130, 65, 90, 100, 150, 2, 86,
            102, 152, 86, 31, 42, 130, 71, 87, 42, 149, 77, 19, 111, 152, 87, 64, 103, 158, 70, 19,
            126, 148, 79, 67, 101, 131, 2, 90, 100, 146, 75, 87, 99, 149, 87, 93, 126, 209, 87, 71,
            42, 157, 67, 81, 101, 131, 71, 19, 111, 133, 2, 87, 101, 157, 77, 65, 111, 209, 79, 82,
            109, 159, 67, 19, 107, 157, 75, 66, 127, 144, 12, 19, 95
        ];
        "125"
    )]
    // "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
    // tempor incididunt ut labore et dolore magna aliqua. Ut"
    #[test_case(
        vec![
            76, 111, 114, 101, 109, 32, 105, 112, 115, 117, 109, 32, 100, 111, 108, 111, 114, 32,
            115, 105, 116, 32, 97, 109, 101, 116, 44, 32, 99, 111, 110, 115, 101, 99, 116, 101, 116,
            117, 114, 32, 97, 100, 105, 112, 105, 115, 99, 105, 110, 103, 32, 101, 108, 105, 116,
             44, 32, 115, 101, 100, 32, 100, 111, 32, 101, 105, 117, 115, 109, 111, 100, 32, 116,
             101, 109, 112, 111, 114, 32, 105, 110, 99, 105, 100, 105, 100, 117, 110, 116, 32, 117,
             116, 32, 108, 97, 98, 111, 114, 101, 32, 101, 116, 32, 100, 111, 108, 111, 114, 101,
             32, 109, 97, 103, 110, 97, 32, 97, 108, 105, 113, 117, 97, 46, 32, 85, 116
        ] =>
        vec![
            130, 254, 0, 126, 10, 241, 34, 51, 70, 158, 80, 86, 103, 209, 75, 67, 121, 132, 79, 19,
            110, 158, 78, 92, 120, 209, 81, 90, 126, 209, 67, 94, 111, 133, 14, 19, 105, 158, 76,
            64, 111, 146, 86, 86, 126, 132, 80, 19, 107, 149, 75, 67, 99, 130, 65, 90, 100, 150, 2,
            86, 102, 152, 86, 31, 42, 130, 71, 87, 42, 149, 77, 19, 111, 152, 87, 64, 103, 158, 70,
            19, 126, 148, 79, 67, 101, 131, 2, 90, 100, 146, 75, 87, 99, 149, 87, 93, 126, 209, 87,
            71, 42, 157, 67, 81, 101, 131, 71, 19, 111, 133, 2, 87, 101, 157, 77, 65, 111, 209, 79,
            82, 109, 159, 67, 19, 107, 157, 75, 66, 127, 144, 12, 19, 95, 133
        ];
        "126"
    )]
    fn test_encode_vec(mut input: Vec<u8>) -> Vec<u8> {
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
        };
        let mask = [0x0a, 0xf1, 0x22, 0x33];

        frame.encode_vec(&mut input, mask);

        input
    }

    #[test_case(&[], ""; "empty slice")]
    #[test_case(b"Hello, world!", "Hello, world!"; "ascii")]
    #[test_case(&[0xC3, 0xA9], "é"; "valid two-byte sequence")]
    #[test_case(&[0xE2, 0x82, 0xAC], "€"; "valid three-byte sequence")]
    #[test_case(&[0xF0, 0x9F, 0xA6, 0x80], "🦀"; "valid four-byte sequence")]
    #[test_case(b"Hello \xC3\xA9\xE2\x82\xAC\xF0\x9F\xA6\x80!", "Hello é€🦀!"; "mixed valid sequences")]
    #[test_case(&[0xF4, 0x8F, 0xBF, 0xBF], "\u{10FFFF}"; "maximum code point")]
    #[test_case(&[0xEF, 0xBF, 0xBF], "\u{FFFF}"; "last valid three-byte sequence")]
    fn test_valid_utf8(input: &[u8], expected: &str) {
        assert_eq!(Frame::validate_utf8(input), Some(expected));
    }

    #[test_case(&[0x80]; "continuation byte without start byte")]
    #[test_case(&[0xFF]; "invalid start byte")]
    #[test_case(&[0xC3]; "incomplete two-byte sequence")]
    #[test_case(&[0xE2, 0x82]; "incomplete three-byte sequence")]
    #[test_case(&[0xF0, 0x9F, 0xA6]; "incomplete four-byte sequence")]
    #[test_case(&[0xC1, 0x81]; "overlong encoding")]
    #[test_case(&[0xED, 0xA0, 0x80]; "surrogate code point")]
    #[test_case(&[0xF5, 0x90, 0x80, 0x80]; "beyond maximum code point")]
    #[test_case(b"Hello \xC3\xA9\xFF"; "mixed valid invalid")]
    #[test_case(&[0xF4, 0x90, 0x80, 0x80]; "just beyond maximum code point")]
    fn test_invalid_utf8(input: &[u8]) {
        assert_eq!(Frame::validate_utf8(input), None);
    }
}

use std::ptr;

use crate::Opcode;

// 2 byte header + 4 byte masking key.
const CONTROL_HEADER_LEN: usize = 6;
const MAX_HEADER_LEN: usize = 14;
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
pub struct Frame<'a> {
    pub fin: bool,
    pub opcode: Opcode,
    pub data: &'a [u8],
}

impl<'a> Frame<'a> {
    pub const CONTROL_HEADER_LEN: usize = CONTROL_HEADER_LEN;
    pub const MAX_HEADER_LEN: usize = MAX_HEADER_LEN;

    #[must_use]
    pub fn binary(data: &'a [u8]) -> Self {
        Self {
            fin: true,
            opcode: Opcode::Binary,
            data,
        }
    }

    #[must_use]
    pub fn text(data: &'a str) -> Self {
        Self {
            fin: true,
            opcode: Opcode::Text,
            data: data.as_bytes(),
        }
    }

    #[inline]
    #[expect(clippy::uninit_vec)]
    pub fn encode_control(self, dst: &mut Vec<u8>, mask: [u8; 4]) {
        let src = self.data;
        let data_len = src.len();
        let len = CONTROL_HEADER_LEN + data_len;

        // SAFE IMPL
        // dst.resize(len, 0);

        // dst[0] = ((self.fin as u8) << 7) | self.opcode as u8;
        // dst[1] = MASK_BIT | data_len as u8;

        // dst[2..6].copy_from_slice(&mask);

        // for i in 0..src.len() {
        //     dst[i + CONTROL_HEADER_LEN] = src[i] ^ mask[i & 3];
        // }

        // UNSAFE IMPL
        dst.reserve(len);
        unsafe {
            dst.set_len(len);

            let src = src.as_ptr();
            let dst = dst.as_mut_ptr();

            dst.write(((self.fin as u8) << 7) | self.opcode as u8);
            dst.add(1).write(MASK_BIT | data_len as u8);
            ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(2), mask.len());
            mask_data(src, dst.add(6), data_len, mask);
        }
    }

    #[inline]
    #[expect(clippy::uninit_vec)]
    pub fn encode(self, dst: &mut Vec<u8>, mask: [u8; 4]) {
        let src = self.data;
        let data_len = src.len();
        let header_len = match data_len {
            ..126 => 6,
            126..65536 => 8,
            _ => 14,
        };
        let len = header_len + data_len;

        // SAFE IMPL
        // dst.resize(len, 0);

        // dst[0] = ((self.fin as u8) << 7) | self.opcode as u8;

        // match header_len {
        //     6 => {
        //         dst[1] = MASK_BIT | data_len as u8;
        //         dst[2..6].copy_from_slice(&mask);
        //     }
        //     8 => {
        //         let data_len_bytes = (data_len as u16).to_be_bytes();
        //         dst[1] = MASK_BIT | 126;
        //         dst[2..4].copy_from_slice(&data_len_bytes);
        //         dst[4..8].copy_from_slice(&mask);
        //     }
        //     14 => {
        //         let data_len_bytes = (data_len as u64).to_be_bytes();
        //         dst[1] = MASK_BIT | 127;
        //         dst[2..10].copy_from_slice(&data_len_bytes);
        //         dst[10..14].copy_from_slice(&mask);
        //     }
        //     _ => unreachable!(),
        // }

        // for i in 0..data_len {
        //     dst[i + header_len] = src[i] ^ mask[i & 3];
        // }

        // UNSAFE IMPL
        dst.reserve(len);
        unsafe {
            dst.set_len(len);

            let src = src.as_ptr();
            let dst = dst.as_mut_ptr();

            dst.write(((self.fin as u8) << 7) | self.opcode as u8);
            match header_len {
                6 => {
                    dst.add(1).write(MASK_BIT | data_len as u8);
                    ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(2), mask.len());
                }
                8 => {
                    dst.add(1).write(MASK_BIT | 126);
                    let data_len_bytes = (data_len as u16).to_be_bytes();
                    ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dst.add(2),
                        data_len_bytes.len(),
                    );
                    ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(4), mask.len());
                }
                14 => {
                    dst.add(1).write(MASK_BIT | 127);
                    let data_len_bytes = (data_len as u64).to_be_bytes();
                    ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dst.add(2),
                        data_len_bytes.len(),
                    );
                    ptr::copy_nonoverlapping(mask.as_ptr(), dst.add(10), mask.len());
                }
                _ => unreachable!(),
            }
            mask_data(src, dst.add(header_len), data_len, mask);
        }
    }

    #[inline]
    #[must_use]
    pub fn validate_utf8(data: &[u8]) -> Option<&str> {
        simdutf8::basic::from_utf8(data).ok()
    }
}

unsafe fn mask_data(src: *const u8, dst: *mut u8, len: usize, mask: [u8; 4]) {
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            if len >= 16 && is_x86_feature_detected!("ssse3") {
                return mask_simd_x86(src, dst, len, mask);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if len >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
                return mask_simd_aarch(src, dst, len, mask);
            }
        }
        mask_scalar(src, dst, len, mask);
    }
}

#[inline]
unsafe fn mask_scalar(src: *const u8, dst: *mut u8, len: usize, mask: [u8; 4]) {
    for i in 0..len {
        unsafe {
            dst.add(i)
                .write(src.add(i).read() ^ mask.get_unchecked(i & 3));
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
#[inline]
unsafe fn mask_simd_x86(src: *const u8, dst: *mut u8, len: usize, mask: [u8; 4]) {
    use std::{
        arch::x86_64::{__m128i, _mm_loadu_si128, _mm_set1_epi32, _mm_storeu_si128, _mm_xor_si128},
        mem,
    };

    let chunks = len / 16;
    unsafe {
        // Handle full chunks with SIMD.
        let mask_value = mem::transmute::<[u8; 4], i32>(mask);
        let mask_x4 = _mm_set1_epi32(mask_value);
        for i in 0..chunks {
            let i = i * 16;
            let src = _mm_loadu_si128(src.add(i) as *const __m128i);
            let masked = _mm_xor_si128(src, mask_x4);
            _mm_storeu_si128(dst.add(i).cast::<__m128i>(), masked);
        }

        // Handle remaining bytes first individually.
        for i in chunks * 16..len {
            dst.add(i)
                .write(src.add(i).read() ^ mask.get_unchecked(i & 3));
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn mask_simd_aarch(src: *const u8, dst: *mut u8, len: usize, mask: [u8; 4]) {
    use std::{
        arch::aarch64::{uint8x16_t, uint32x4_t, vdupq_n_u32, veorq_u8, vld1q_u8, vst1q_u8},
        mem,
    };

    let chunks = len / 16;
    unsafe {
        // Handle full chunks with SIMD.
        let mask_value = mem::transmute::<[u8; 4], u32>(mask);
        let mask_x4 = mem::transmute::<uint32x4_t, uint8x16_t>(vdupq_n_u32(mask_value));
        for i in 0..chunks {
            let i = i * 16;
            let src = vld1q_u8(src.add(i).cast_const());
            let masked = veorq_u8(src, mask_x4);
            vst1q_u8(dst.add(i), masked);
        }

        // Handle remaining bytes first individually.
        for i in chunks * 16..len {
            dst.add(i)
                .write(src.add(i).read() ^ mask.get_unchecked(i & 3));
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
    fn test_encode_control(input: Vec<u8>) -> Vec<u8> {
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
            data: &input,
        };
        let mask = [0x0a, 0xf1, 0x22, 0x33];
        let mut output = Vec::with_capacity(input.len() + Frame::CONTROL_HEADER_LEN);

        frame.encode_control(&mut output, mask);

        output
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
    fn test_encode_vec(input: Vec<u8>) -> Vec<u8> {
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
            data: &input,
        };
        let mask = [0x0a, 0xf1, 0x22, 0x33];
        let mut output = Vec::with_capacity(input.len() + Frame::MAX_HEADER_LEN);

        frame.encode(&mut output, mask);

        output
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

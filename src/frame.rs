use crate::Opcode;

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
    // 2 byte header + 4 byte masking key.
    pub const CONTROL_HEADER_LEN: usize = 6;
    const MASK_BIT: u8 = 0x80;

    /// Header space needs to be pre-allocated for the slice!
    #[inline]
    pub fn encode_control_slice(self, data: &mut [u8], mask: [u8; 4]) {
        let data_len = data.len() - Self::CONTROL_HEADER_LEN;

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
            let dist = data.as_mut_ptr();
            // Assumes data is contained in the 0..data.len() - 6 part.
            for i in (Self::CONTROL_HEADER_LEN..data.len()).rev() {
                let j = i - Self::CONTROL_HEADER_LEN;
                dist.add(i)
                    .write(data.get_unchecked(j) ^ mask.get_unchecked(j & 3));
            }

            dist.write(((self.fin as u8) << 7) | self.opcode as u8);
            dist.add(1).write(Self::MASK_BIT | data_len as u8);

            std::ptr::copy_nonoverlapping(mask.as_ptr(), dist.add(2), mask.len());
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
            let dist = data.as_mut_ptr();
            for i in (header_len..data.len()).rev() {
                let j = i - header_len;
                dist.add(i)
                    .write(data.get_unchecked(j) ^ mask.get_unchecked(j & 3));
            }

            dist.write(((self.fin as u8) << 7) | self.opcode as u8);

            match header_len {
                6 => {
                    dist.add(1).write(Self::MASK_BIT | data_len as u8);
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dist.add(2), mask.len());
                }
                8 => {
                    let data_len_bytes = (data_len as u16).to_be_bytes();
                    dist.add(1).write(Self::MASK_BIT | 126);
                    std::ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dist.add(2),
                        data_len_bytes.len(),
                    );
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dist.add(4), mask.len());
                }
                14 => {
                    let data_len_bytes = (data_len as u64).to_be_bytes();
                    dist.add(1).write(Self::MASK_BIT | 127);
                    std::ptr::copy_nonoverlapping(
                        data_len_bytes.as_ptr(),
                        dist.add(2),
                        data_len_bytes.len(),
                    );
                    std::ptr::copy_nonoverlapping(mask.as_ptr(), dist.add(10), mask.len());
                }
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(
        // "hello"
        vec![0x68, 0x65, 0x6C, 0x6C, 0x6F] =>
        vec![130, 133, 10, 241, 34, 51, 98, 148, 78, 95, 101];
        "short"
    )]
    #[test_case(
        // "hello world"
        vec![104, 101, 108, 108, 111, 32, 119, 111, 114, 108, 100] =>
        vec![130, 139, 10, 241, 34, 51, 98, 148, 78, 95, 101, 209, 85, 92, 120, 157, 70];
        "long"
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
        "short"
    )]
    #[test_case(
        vec![0; 126] =>
        vec![
            130, 254, 0, 126, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51,
            10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10,
            241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10,
            241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10,
            241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10,
            241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10,
            241, 34, 51, 10, 241, 34, 51, 10, 241, 34, 51, 10, 241
        ];
        "extended 1"
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
}

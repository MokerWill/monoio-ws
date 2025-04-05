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
    // pub data_len: usize;
}

impl Frame {
    // 2 byte header + 4 byte masking key.
    pub const CONTROL_HEADER_LEN: usize = 6;
    const MASK_BIT: u8 = 0x80;

    /// Header space needs to be pre-allocated for the slice!
    #[inline]
    pub fn encode_control_slice(self, data: &mut [u8], mask: [u8; 4]) {
        let data_len = data.len() - Self::CONTROL_HEADER_LEN;

        // Assumes data is contained in the 0..data.len() - 6 part.
        for i in (Self::CONTROL_HEADER_LEN..data.len()).rev() {
            let j = i - Self::CONTROL_HEADER_LEN;
            data[i] = data[j] ^ mask[j & 3];
        }

        let [a, b, c, d] = mask;
        data[5] = d;
        data[4] = c;
        data[3] = b;
        data[2] = a;

        data[1] = Self::MASK_BIT | data_len as u8;
        data[0] = ((self.fin as u8) << 7) | self.opcode as u8;
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

        for i in (header_len..data.len()).rev() {
            let j = i - header_len;
            data[i] = data[j] ^ mask[j & 3];
        }

        let [a, b, c, d] = mask;
        match header_len {
            6 => {
                data[5] = d;
                data[4] = c;
                data[3] = b;
                data[2] = a;
                data[1] = Self::MASK_BIT | data_len as u8;
            }
            8 => {
                let [b2, b3] = (data_len as u16).to_be_bytes();
                data[7] = d;
                data[6] = c;
                data[5] = b;
                data[4] = a;
                data[3] = b3;
                data[2] = b2;
                data[1] = Self::MASK_BIT | 126;
            }
            14 => {
                let [b2, b3, b4, b5, b6, b7, b8, b9] = (data_len as u64).to_be_bytes();
                data[13] = d;
                data[12] = c;
                data[11] = b;
                data[10] = a;
                data[9] = b9;
                data[8] = b8;
                data[7] = b7;
                data[6] = b6;
                data[5] = b5;
                data[4] = b4;
                data[3] = b3;
                data[2] = b2;
                data[1] = Self::MASK_BIT | 127;
            }
            _ => unreachable!(),
        }
        data[0] = ((self.fin as u8) << 7) | self.opcode as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode() {
        let mask = [0x0a, 0xf1, 0x22, 0x33];
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
        };

        // "hello"
        let mut a = vec![0x68, 0x65, 0x6C, 0x6C, 0x6F];
        let mut b = a.clone();

        frame.encode_vec(&mut a, mask);
        b.resize(b.len() + Frame::CONTROL_HEADER_LEN, 0);
        frame.encode_control_slice(&mut b, mask);

        assert_eq!(a, b);
    }
}

use std::io;

use monoio::{
    BufResult,
    buf::{IoBufMut, Slice, SliceMut},
    io::{AsyncReadRent, AsyncWriteRent},
};

pub trait AsyncReadRentExt {
    type T;

    fn read_extend(
        &mut self,
        buf: Self::T,
        len: usize,
    ) -> impl Future<Output = BufResult<usize, Self::T>>;
}

impl<A> AsyncReadRentExt for A
where
    A: AsyncReadRent,
{
    type T = Vec<u8>;

    async fn read_extend(&mut self, mut buf: Self::T, len: usize) -> BufResult<usize, Self::T> {
        let offset = buf.len();
        let end = offset + len;

        buf.reserve(len);

        let mut read = 0;
        while read < len {
            let buf_slice = unsafe { SliceMut::new_unchecked(buf, offset + read, end) };
            let (result, buf_slice) = self.read(buf_slice).await;
            buf = buf_slice.into_inner();
            match result {
                Ok(0) => {
                    return (
                        Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "failed to fill whole buffer",
                        )),
                        buf,
                    );
                }
                Ok(n) => {
                    read += n;
                    unsafe { buf.set_init(offset + read) };
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            }
        }
        (Ok(read), buf)
    }
}

pub trait AsyncWriteRentExt {
    type T;

    fn write_offset(
        &mut self,
        buf: Self::T,
        offset: usize,
    ) -> impl Future<Output = BufResult<usize, Self::T>>;
}

impl<A> AsyncWriteRentExt for A
where
    A: AsyncWriteRent,
{
    type T = Vec<u8>;

    async fn write_offset(&mut self, mut buf: Self::T, offset: usize) -> BufResult<usize, Self::T> {
        let len = buf.len() - offset;
        let mut written = 0;
        while written < len {
            let buf_slice = unsafe { Slice::new_unchecked(buf, offset + written, offset + len) };
            let (result, buf_slice) = self.write(buf_slice).await;
            buf = buf_slice.into_inner();
            match result {
                Ok(0) => {
                    return (
                        Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "failed to write whole buffer",
                        )),
                        buf,
                    );
                }
                Ok(n) => written += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            }
        }
        (Ok(written), buf)
    }
}

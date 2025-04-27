use std::io;

use monoio::{
    BufResult,
    buf::{IoBufMut, SliceMut},
    io::AsyncReadRent,
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

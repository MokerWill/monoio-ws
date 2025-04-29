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

        buf.reserve(end);

        let buf_slice = unsafe { SliceMut::new_unchecked(buf, offset, end) };
        let (result, buf_slice) = self.read(buf_slice).await;
        buf = buf_slice.into_inner();
        match result {
            Ok(read) => {
                unsafe { buf.set_init(offset + read) };
                (Ok(read), buf)
            }
            Err(e) => (Err(e), buf),
        }
    }
}

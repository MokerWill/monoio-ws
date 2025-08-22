use bumpalo::Bump;
use std::cell::UnsafeCell;

pub struct Arena {
    bump: UnsafeCell<Bump>,
}

unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

impl Arena {
    pub fn new() -> Self {
        Self {
            bump: UnsafeCell::new(Bump::new()),
        }
    }

    #[inline]
    pub fn alloc_slice_copy(&self, data: &[u8]) -> &[u8] {
        let bump = unsafe { &mut *self.bump.get() };
        bump.alloc_slice_copy(data)
    }

    #[inline]
    pub fn alloc_slice_fill_copy(&self, len: usize, value: u8) -> &mut [u8] {
        let bump = unsafe { &mut *self.bump.get() };
        bump.alloc_slice_fill_copy(len, value)
    }

    #[inline]
    pub fn reset(&self) {
        let bump = unsafe { &mut *self.bump.get() };
        bump.reset();
    }

    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bump: UnsafeCell::new(Bump::with_capacity(capacity)),
        }
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}
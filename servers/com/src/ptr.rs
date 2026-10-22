use std::marker::PhantomData;

use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct RawPtr<T> {
    addr: u64,
    phantom: PhantomData<T>
}

unsafe impl<T> Sync for RawPtr<T> {}
unsafe impl<T> Send for RawPtr<T> {}

impl<T> Default for RawPtr<T> {
    fn default() -> Self {
        Self { addr:0, phantom: Default::default() }
    }
}

impl<T> RawPtr<T> {
    pub fn new(val: T) -> Self {
        let b = Box::new(val);
        let ret = Self { addr: &*b as *const _ as u64, phantom: PhantomData };
        std::mem::forget(b);
        ret
    }

    pub fn null() -> Self {
        Self { addr: 0, phantom: PhantomData }
    }

    #[inline]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.addr as *mut u8 as *mut T
    }

    #[inline]
    pub fn as_mut(&self) -> &mut T {
        unsafe { &mut *self.as_mut_ptr() }
    }
}

impl<T> From<*mut T> for RawPtr<T> {
    fn from(value: *mut T) -> Self {
        Self { addr: value as u64, phantom: PhantomData }
    }
}
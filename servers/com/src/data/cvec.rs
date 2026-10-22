use std::{
    fmt, mem, slice,
    marker::PhantomData,
    ops::{Index, IndexMut},
    ptr::{self, slice_from_raw_parts},
};

use serde::{Deserialize, Serialize};

use crate::ptr::RawPtr;

#[repr(C)]
struct CVecInner<T> {
    begin: *mut T,
    end: *mut T,
    cap: *mut T,
}

impl<T> CVecInner<T> {
    #[inline]
    pub fn len(&self) -> usize {
        unsafe { self.end.offset_from(self.begin) as usize }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.end == self.begin
    }

    #[inline]
    pub fn push(&mut self, val: T, cap: usize) {
        unsafe {
            if self.end == self.cap {
                // Make room for new data (1/8 of total capacity)
                self.shift(cap >> 3);
            }
            ptr::write(self.end, val);
            self.end = self.end.add(1);
        }
    }

    #[inline]
    pub fn append(&mut self, vec: &Vec<T>, cap: usize) {
        unsafe {
            assert!(
                vec.len() < cap,
                "Vec (size={}) does not fit into CVec (cap={})",
                vec.len(),
                cap
            );
            if self.end == self.cap || self.end.add(vec.len()) >= self.cap {
                // Make room for new data (1/8 of total capacity)
                self.shift((cap >> 3).max(vec.len()));
            }
            ptr::copy_nonoverlapping(vec.as_ptr(), self.end, vec.len());
            self.end = self.end.add(vec.len());
        }
    }

    #[inline]
    pub fn at(&self, index: usize) -> &T {
        unsafe {
            let item = self.begin.add(index);
            assert!(
                item < self.end,
                "Index out of bound [size: {}, index: {}]",
                self.len(),
                index
            );
            &*item
        }
    }

    #[inline]
    pub fn at_mut(&mut self, index: usize) -> &mut T {
        unsafe {
            let item = self.begin.add(index);
            assert!(
                item < self.end,
                "Index out of bound [size: {}, index: {}]",
                self.len(),
                index
            );
            &mut *item
        }
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        unsafe {
            if self.begin == self.end {
                None
            } else {
                Some(&*self.end.sub(1))
            }
        }
    }

    #[inline]
    pub fn last_mut(&self) -> Option<&mut T> {
        unsafe {
            if self.begin == self.end {
                None
            } else {
                Some(&mut *self.end.sub(1))
            }
        }
    }

    // Return the index of the first element greater than `val`
    unsafe fn upper_bound<U>(&self, val: U) -> usize
    where
        T: PartialOrd<U>,
    {
        let mut first = self.begin;
        let mut count = self.len();
        while count > 0 {
            let mut it = first;
            let step = count / 2;
            it = it.add(step);
            if *it <= val {
                it = it.add(1);
                first = it;
                count -= step + 1;
            } else {
                count = step;
            }
        }
        first.offset_from(self.begin) as usize
    }

    // Return the index of the first element not smaller than `val`
    unsafe fn lower_bound<U>(&self, val: U) -> usize
    where
        T: PartialOrd<U>,
    {
        let mut first = self.begin;
        let mut count = self.len();
        while count > 0 {
            let mut it = first;
            let step = count / 2;
            it = it.add(step);
            if *it < val {
                it = it.add(1);
                first = it;
                count -= step + 1;
            } else {
                count = step;
            }
        }
        first.offset_from(self.begin) as usize
    }

    // Returns a subset of vector starting from `start` and ending with `end`
    pub unsafe fn subset<U>(&self, start: Option<U>, end: Option<U>) -> Vec<T>
    where
        T: Copy + PartialOrd<U>,
    {
        if self.is_empty() {
            vec![]
        } else {
            let lo = start.map(|v| self.lower_bound(v)).unwrap_or(0);
            let hi = end.map(|v| self.upper_bound(v)).unwrap_or(self.len());
            slice::from_raw_parts(self.begin.add(lo), hi - lo).to_vec()
        }
    }

    #[inline]
    unsafe fn shift(&mut self, count: usize) {
        assert!(count <= self.len(), "cannot shift more than size ({count} > {})", self.len());
        ptr::copy(self.begin.add(count), self.begin, self.len() - count + 1);
        self.end = self.end.sub(count);
    }
}

#[derive(Serialize, Deserialize)]
pub struct CVec<T> {
    inner: RawPtr<CVecInner<T>>,
    cap: usize,
}

impl<T: fmt::Debug> fmt::Debug for CVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let slice = unsafe { &*slice_from_raw_parts(self.inner().begin, self.len()) };
        fmt::Debug::fmt(slice, f)
    }
}

impl<T> CVec<T> {
    /// Return the size of vector stored in a file in bytes
    #[inline]
    pub fn size_of(cap: usize) -> usize {
        32 + cap * mem::size_of::<T>()
    }

    pub unsafe fn new(base: *mut u8, trunc: bool, cap: usize) -> Self {
        let mut ret = Self {
            inner: RawPtr::null(),
            cap,
        };
        ret.reconstruct_from_file(base, trunc, cap);
        ret
    }

    #[inline]
    fn inner(&self) -> &mut CVecInner<T> {
        self.inner.as_mut()
    }

    #[inline]
    pub unsafe fn reconstruct_from_file(&mut self, base: *mut u8, trunc: bool, cap: usize) {
        let pointers = base as *mut usize;
        if trunc {
            ptr::write_bytes(base, 0, 32);
            *pointers.add(1) = 32;
            *pointers.add(2) = 32;
            *pointers.add(3) = 32 + cap * mem::size_of::<T>();
        }
        let old_base = *pointers;
        let new_base = base as usize;
        *pointers = new_base;
        *pointers.add(1) = *pointers.add(1) - old_base + new_base;
        *pointers.add(2) = *pointers.add(2) - old_base + new_base;
        *pointers.add(3) = *pointers.add(3) - old_base + new_base;

        use libc::*;
        msync(base as *mut c_void, 32, MS_SYNC | MS_INVALIDATE);
        self.inner = RawPtr::from(pointers.add(1) as *mut CVecInner<T>);
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner().len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner().is_empty()
    }

    #[inline]
    pub fn push(&mut self, val: T) {
        self.inner().push(val, self.cap)
    }

    #[inline]
    pub fn append(&mut self, vec: &Vec<T>) {
        self.inner().append(vec, self.cap)
    }

    #[inline]
    pub fn at(&self, index: usize) -> &T {
        self.inner().at(index)
    }

    #[inline]
    pub fn at_mut(&mut self, index: usize) -> &mut T {
        self.inner().at_mut(index)
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.inner().last()
    }

    #[inline]
    pub fn last_mut(&self) -> Option<&mut T> {
        self.inner().last_mut()
    }

    /// Returns the index of the first element greater than `val`
    ///
    /// # Safety
    /// The vector should be sorted. Otherwise, it may hang.
    pub unsafe fn upper_bound(&self, val: T) -> usize
    where
        T: std::cmp::PartialOrd,
    {
        self.inner().upper_bound(val)
    }

    /// Returns the index of the first element not smaller than `val`
    ///
    /// # Safety
    /// The vector should be sorted. Otherwise, it may hang.
    pub unsafe fn lower_bound(&self, val: T) -> usize
    where
        T: std::cmp::PartialOrd,
    {
        self.inner().lower_bound(val)
    }

    /// Returns a subset of vector starting from `start` and ending with `end`
    ///
    /// # Safety
    /// The vector should be sorted. Otherwise, it may hang.
    pub unsafe fn subset<U>(&self, start: Option<U>, end: Option<U>) -> Vec<T>
    where
        T: Copy + PartialOrd<U>,
    {
        self.inner().subset(start, end)
    }

    #[inline]
    pub(crate) unsafe fn next_base(&self) -> *mut u8 {
        self.inner().cap.add(1) as *mut u8
    }
}

pub struct IntoIter<T> {
    ptr: *mut T,
    end: *mut T,
}

unsafe impl<T: Send> Send for IntoIter<T> {}
unsafe impl<T: Sync> Sync for IntoIter<T> {}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            let old = self.ptr;
            self.ptr = unsafe { self.ptr.add(1) };

            Some(unsafe { ptr::read(old) })
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let exact = unsafe { self.end.offset_from(self.ptr) as usize };
        (exact, Some(exact))
    }

    #[inline]
    fn count(self) -> usize {
        self.size_hint().0
    }
}

impl<T> IntoIterator for CVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            ptr: self.inner().begin,
            end: self.inner().end,
        }
    }
}

pub struct RefIter<'a, T> {
    ptr: *mut T,
    end: *mut T,
    _marker: PhantomData<&'a T>,
}

unsafe impl<T: Send> Send for RefIter<'_, T> {}
unsafe impl<T: Sync> Sync for RefIter<'_, T> {}

impl<'a, T> Iterator for RefIter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end as *mut T {
            None
        } else {
            let old = self.ptr;
            self.ptr = unsafe { self.ptr.add(1) };

            Some(unsafe { &*old })
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let exact = (self.end as *mut T as usize - self.ptr as usize) / mem::size_of::<T>();
        (exact, Some(exact))
    }

    #[inline]
    fn count(self) -> usize {
        self.size_hint().0
    }
}

impl<'a, T> IntoIterator for &'a CVec<T> {
    type Item = &'a T;
    type IntoIter = RefIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        RefIter {
            ptr: self.inner().begin,
            end: self.inner().end,
            _marker: PhantomData,
        }
    }
}

pub struct RefIterMut<'a, T> {
    ptr: *mut T,
    end: *mut T,
    _marker: PhantomData<&'a mut T>,
}

unsafe impl<T: Send> Send for RefIterMut<'_, T> {}
unsafe impl<T: Sync> Sync for RefIterMut<'_, T> {}

impl<'a, T> Iterator for RefIterMut<'a, T> {
    type Item = &'a mut T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end as *mut T {
            None
        } else {
            let old = self.ptr;
            self.ptr = unsafe { self.ptr.add(1) };

            Some(unsafe { &mut *old })
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let exact = (self.end as *mut T as usize - self.ptr as usize) / mem::size_of::<T>();
        (exact, Some(exact))
    }

    #[inline]
    fn count(self) -> usize {
        self.size_hint().0
    }
}

impl<'a, T> IntoIterator for &'a mut CVec<T> {
    type Item = &'a mut T;
    type IntoIter = RefIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        RefIterMut {
            ptr: self.inner().begin,
            end: self.inner().end,
            _marker: PhantomData,
        }
    }
}

impl<T> Index<usize> for CVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.at(index)
    }
}

impl<T> IndexMut<usize> for CVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.at_mut(index)
    }
}

mod test {

    #[allow(dead_code)]
    fn map_file(path: &str) -> *mut u8 {
        use std::fs::OpenOptions;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .unwrap();
        let mut len = file.metadata().unwrap().len() as usize;
        if len < 8 {
            len = 10 * 1024;
            file.set_len(len as u64).unwrap();
        }

        let mut memmap = unsafe { memmap2::MmapOptions::new().map_mut(&file).unwrap() };
        let ret = memmap.as_mut_ptr();
        std::mem::forget(file);
        std::mem::forget(memmap);
        ret
    }

    #[test]
    fn create_push() {
        let ptr = map_file("memory.bin");
        let mut vec = unsafe { super::CVec::<i32>::new(ptr, true, 10) };

        let data = [10, 20, 30, 40, 50];
        for d in data {
            vec.push(d);
        }
        for i in 0..data.len() {
            assert_eq!(vec[i], data[i])
        }
        println!("{:p} {:?}", ptr, vec)
    }

    #[test]
    #[ignore = "needs a pre-populated memory.bin holding [10, 20, 30, 40, 50]; not self-contained"]
    fn load() {
        let ptr = map_file("memory.bin");
        let vec = unsafe { super::CVec::<i32>::new(ptr, true, 10) };

        let data = [10, 20, 30, 40, 50];
        for i in 0..data.len() {
            assert_eq!(vec[i], data[i])
        }
        // vec.push(data[vec.len() % data.len()]);
        // vec.push(vec[0]);
        println!("{:p} {:?}", ptr, vec)
    }

    #[test]
    fn bounds() {
        let mut data = [0i32; 256];
        let ptr: *mut i32 = data.as_mut_ptr();
        let mut vec = unsafe { super::CVec::<i32>::new(ptr as *mut u8, true, data.len()) };
        vec.append(&vec![
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140,
        ]);
        unsafe {
            let i = vec.upper_bound(50);
            assert_eq!(vec[i], 60);
            let i = vec.upper_bound(39);
            assert_eq!(vec[i], 40);
            let i = vec.lower_bound(50);
            assert_eq!(vec[i], 50);
            let i = vec.lower_bound(39);
            assert_eq!(vec[i], 40);
            println!("{:?}", vec.subset(None, Some(123)));
            println!("{:?}", vec.subset(Some(55), None));
            println!("{:?}", vec.subset(Some(55), Some(123)));
            println!("{:?}", vec.subset(Some(55), Some(1000)));
        }
    }
}

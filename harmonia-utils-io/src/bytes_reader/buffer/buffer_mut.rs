#![allow(unsafe_code)]

use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::{self, NonNull};
use std::slice;
use std::sync::Arc;

use bytes::{Buf, BufMut};

pub struct Filled {
    ptr: NonNull<u8>,
    len: usize,
    _data: Arc<Vec<u8>>,
}

impl Filled {
    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl AsRef<[u8]> for Filled {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

unsafe impl Send for Filled {}
unsafe impl Sync for Filled {}

pub struct BufferMut {
    ptr: NonNull<u8>,
    len: usize,
    cap: usize,
    data: Arc<Vec<u8>>,
}

impl BufferMut {
    #[inline]
    pub fn with_capacity(capacity: usize) -> BufferMut {
        BufferMut::from_vec(Vec::with_capacity(capacity))
    }

    // private

    // For now, use a `Vec` to manage the memory for us, but we may want to
    // change that in the future to some alternate allocator strategy.
    //
    // Thus, we don't expose an easy way to construct from a `Vec` since an
    // internal change could make a simple pattern (`BytesMut::from(vec)`)
    // suddenly a lot more expensive.
    #[inline]
    pub(crate) fn from_vec(vec: Vec<u8>) -> BufferMut {
        let mut data = Arc::new(vec);
        let ptr = vptr(Arc::make_mut(&mut data).as_mut_ptr());
        let len = data.len();
        let cap = data.capacity();
        BufferMut {
            ptr,
            len,
            cap,
            data,
        }
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Returns the number of bytes contained in this `BytesMut`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytes::BytesMut;
    ///
    /// let b = BytesMut::from(&b"hello"[..]);
    /// assert_eq!(b.len(), 5);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the `BytesMut` has a length of 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytes::BytesMut;
    ///
    /// let b = BytesMut::with_capacity(64);
    /// assert!(b.is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of bytes the `BytesMut` can hold without reallocating.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytes::BytesMut;
    ///
    /// let b = BytesMut::with_capacity(64);
    /// assert_eq!(b.capacity(), 64);
    /// ```
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Makes room for `additional` more bytes, capped at `limit` in total.
    /// Doubles the allocation unless shifting the held bytes to the front
    /// frees enough, so filling a little at a time copies each byte O(1) times.
    #[inline]
    pub fn reserve(&mut self, additional: usize, limit: usize) {
        let additional = additional.min(limit.saturating_sub(self.len));
        if additional <= self.cap - self.len {
            return;
        }
        self.reserve_inner(additional, limit);
    }

    // In a separate function so the check in `reserve` can be inlined.
    fn reserve_inner(&mut self, additional: usize, limit: usize) {
        let len = self.len;
        // `reserve` keeps this within `limit`.
        let needed = len + additional;
        let size = self.data.capacity();
        // SAFETY: `ptr` points into the allocation of `data`.
        let offset = unsafe { self.ptr.as_ptr().offset_from_unsigned(self.data.as_ptr()) };

        let new_cap = if size >= needed && offset >= len {
            size
        } else {
            size.saturating_mul(2)
        };
        let new_cap = new_cap.min(limit).max(needed);
        if new_cap <= size
            && let Some(v) = Arc::get_mut(&mut self.data)
        {
            let start = v.as_mut_ptr();
            // SAFETY: both ranges are inside `v`.
            unsafe { ptr::copy(self.ptr.as_ptr(), start, len) };
            self.ptr = vptr(start);
            self.cap = size;
            return;
        }
        let mut v = Vec::with_capacity(new_cap);
        v.extend_from_slice(self.as_slice());
        self.ptr = vptr(v.as_mut_ptr());
        self.cap = v.capacity();
        self.data = Arc::new(v);
    }

    #[inline]
    pub fn filled(&self) -> bytes::Bytes {
        let filled = Filled {
            ptr: self.ptr,
            len: self.len,
            _data: self.data.clone(),
        };
        bytes::Bytes::from_owner(filled)
    }

    /// Advance the buffer without bounds checking.
    ///
    /// # SAFETY
    ///
    /// The caller must ensure that `count` <= `self.cap`.
    pub(crate) unsafe fn advance_unchecked(&mut self, count: usize) {
        // Setting the start to 0 is a no-op, so return early if this is the
        // case.
        if count == 0 {
            return;
        }

        debug_assert!(count <= self.cap, "internal: set_start out of bounds");

        // Updating the start of the view is setting `ptr` to point to the
        // new start and updating the `len` field to reflect the new length
        // of the view.
        self.ptr = unsafe { vptr(self.ptr.as_ptr().add(count)) };
        self.len = self.len.saturating_sub(count);
        self.cap -= count;
    }

    /// Returns the remaining spare capacity of the buffer as a slice of `MaybeUninit<u8>`.
    ///
    /// The returned slice can be used to fill the buffer with data (e.g. by
    /// reading from a file) before marking the data as initialized using the
    /// [`set_len`] method.
    ///
    /// [`set_len`]: BytesMut::set_len
    ///
    /// # Examples
    ///
    /// ```
    /// use bytes::BytesMut;
    ///
    /// // Allocate buffer big enough for 10 bytes.
    /// let mut buf = BytesMut::with_capacity(10);
    ///
    /// // Fill in the first 3 elements.
    /// let uninit = buf.spare_capacity_mut();
    /// uninit[0].write(0);
    /// uninit[1].write(1);
    /// uninit[2].write(2);
    ///
    /// // Mark the first 3 bytes of the buffer as being initialized.
    /// unsafe {
    ///     buf.set_len(3);
    /// }
    ///
    /// assert_eq!(&buf[..], &[0, 1, 2]);
    /// ```
    #[inline]
    pub fn spare_capacity_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe {
            let ptr = self.ptr.as_ptr().add(self.len);
            let len = self.cap - self.len;

            slice::from_raw_parts_mut(ptr.cast(), len)
        }
    }
}

impl AsRef<[u8]> for BufferMut {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for BufferMut {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

impl Buf for BufferMut {
    #[inline]
    fn remaining(&self) -> usize {
        self.len()
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        self.as_slice()
    }

    #[inline]
    fn advance(&mut self, cnt: usize) {
        assert!(
            cnt <= self.remaining(),
            "cannot advance past `remaining`: {:?} <= {:?}",
            cnt,
            self.remaining(),
        );
        unsafe {
            // SAFETY: We've checked that `cnt` <= `self.remaining()` and we know that
            // `self.remaining()` <= `self.cap`.
            self.advance_unchecked(cnt);
        }
    }

    fn copy_to_bytes(&mut self, len: usize) -> bytes::Bytes {
        let ret = self.filled().split_to(len);
        self.advance(len);
        ret
    }
}

unsafe impl BufMut for BufferMut {
    fn remaining_mut(&self) -> usize {
        self.capacity() - self.len()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        let remaining = self.cap - self.len();
        if cnt > remaining {
            panic!("advance out of bounds: the len is {remaining} but advancing by {cnt}");
        }
        // Addition won't overflow since it is at most `self.cap`.
        self.len = self.len() + cnt;
    }

    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        self.spare_capacity_mut().into()
    }
}

unsafe impl Send for BufferMut {}
unsafe impl Sync for BufferMut {}

#[inline]
fn vptr(ptr: *mut u8) -> NonNull<u8> {
    if cfg!(debug_assertions) {
        NonNull::new(ptr).expect("Vec pointer should be non-null")
    } else {
        unsafe { NonNull::new_unchecked(ptr) }
    }
}

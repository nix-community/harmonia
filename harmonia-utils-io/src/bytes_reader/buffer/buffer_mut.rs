#![allow(unsafe_code)]

use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::{cmp, slice};

use bytes::{Buf, BufMut};
use tracing::warn;

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

    /// Reserves capacity for at least `additional` more bytes to be inserted
    /// into the given `BytesMut`.
    ///
    /// More than `additional` bytes may be reserved in order to avoid frequent
    /// reallocations. A call to `reserve` may result in an allocation.
    ///
    /// Before allocating new buffer space, the function will attempt to reclaim
    /// space in the existing buffer. If the current handle references a view
    /// into a larger original buffer, and all other handles referencing part
    /// of the same original buffer have been dropped, then the current view
    /// can be copied/shifted to the front of the buffer and the handle can take
    /// ownership of the full buffer, provided that the full buffer is large
    /// enough to fit the requested additional capacity.
    ///
    /// This optimization will only happen if shifting the data from the current
    /// view to the front of the buffer is not too expensive in terms of the
    /// (amortized) time required. The precise condition is subject to change;
    /// as of now, the length of the data being shifted needs to be at least as
    /// large as the distance that it's shifted by. If the current view is empty
    /// and the original buffer is large enough to fit the requested additional
    /// capacity, then reallocations will never happen.
    ///
    /// # Examples
    ///
    /// In the following example, a new buffer is allocated.
    ///
    /// ```
    /// use bytes::BytesMut;
    ///
    /// let mut buf = BytesMut::from(&b"hello"[..]);
    /// buf.reserve(64);
    /// assert!(buf.capacity() >= 69);
    /// ```
    ///
    /// In the following example, the existing buffer is reclaimed.
    ///
    /// ```
    /// use bytes::{BytesMut, BufMut};
    ///
    /// let mut buf = BytesMut::with_capacity(128);
    /// buf.put(&[0; 64][..]);
    ///
    /// let ptr = buf.as_ptr();
    /// let other = buf.split();
    ///
    /// assert!(buf.is_empty());
    /// assert_eq!(buf.capacity(), 64);
    ///
    /// drop(other);
    /// buf.reserve(128);
    ///
    /// assert_eq!(buf.capacity(), 128);
    /// assert_eq!(buf.as_ptr(), ptr);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize`.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        let len = self.len();
        let rem = self.capacity() - len;

        if additional <= rem {
            // The handle can already store at least `additional` more bytes, so
            // there is no further work needed to be done.
            return;
        }

        // will always succeed
        let _ = self.reserve_inner(additional, true);
    }

    // In separate function to allow the short-circuits in `reserve` and `try_reclaim` to
    // be inline-able. Significantly helps performance. Returns false if it did not succeed.
    fn reserve_inner(&mut self, additional: usize, allocate: bool) -> bool {
        let len = self.len();

        // Reserving involves abandoning the currently shared buffer and
        // allocating a new vector with the requested capacity.
        //
        // Compute the new capacity
        let mut new_cap = match len.checked_add(additional) {
            Some(new_cap) => new_cap,
            None if !allocate => return false,
            None => panic!("overflow"),
        };

        if let Some(v) = Arc::get_mut(&mut self.data) {
            unsafe {
                let v_capacity = v.capacity();
                let ptr = v.as_mut_ptr();

                let offset = self.ptr.as_ptr().offset_from(ptr) as usize;

                // Only reuse space if we can satisfy the requested additional space.
                //
                // Also check if the value of `off` suggests that enough bytes
                // have been read to account for the overhead of shifting all
                // the data (in an amortized analysis).
                // Hence the condition `off >= self.len()`.
                //
                // This condition also already implies that the buffer is going
                // to be (at least) half-empty in the end; so we do not break
                // the (amortized) runtime with future resizes of the underlying
                // `Vec`.
                //
                // [For more details check issue bytes#524, and PR bytes#525.]
                if v_capacity >= new_cap && offset >= len {
                    // The capacity is sufficient, and copying is not too much
                    // overhead: reclaim the buffer!

                    // `offset >= len` means: no overlap
                    ptr::copy_nonoverlapping(self.ptr.as_ptr(), ptr, len);

                    self.ptr = vptr(ptr);
                    self.cap = v.capacity();
                } else {
                    if !allocate {
                        return false;
                    }

                    // new_cap is calculated in terms of `BytesMut`, not the underlying
                    // `Vec`, so it does not take the offset into account.
                    //
                    // Thus we have to manually add it here.
                    new_cap = new_cap.checked_add(offset).expect("overflow");

                    // The vector capacity is not sufficient. The reserve request is
                    // asking for more than the initial buffer capacity. Allocate more
                    // than requested if `new_cap` is not much bigger than the current
                    // capacity.
                    //
                    // There are some situations, using `reserve_exact` that the
                    // buffer capacity could be below `original_capacity`, so do a
                    // check.
                    let double = v.capacity().checked_shl(1).unwrap_or(new_cap);

                    new_cap = cmp::max(double, new_cap);

                    // No space - allocate more
                    //
                    // The length field of `Shared::vec` is not used by the `BytesMut`;
                    // instead we use the `len` field in the `BytesMut` itself. However,
                    // when calling `reserve`, it doesn't guarantee that data stored in
                    // the unused capacity of the vector is copied over to the new
                    // allocation, so we need to ensure that we don't have any data we
                    // care about in the unused capacity before calling `reserve`.
                    debug_assert!(offset + len <= v.capacity());
                    v.set_len(offset + len);
                    v.reserve(new_cap - v.len());

                    // Update the info
                    self.ptr = vptr(v.as_mut_ptr().add(offset));
                    self.cap = v.capacity() - offset;
                }
            }

            return true;
        }
        if !allocate {
            return false;
        }
        warn!(additional, "buffer is still shared");

        new_cap = cmp::max(new_cap, self.data.capacity());

        // Create a new vector to store the data
        let mut v = Vec::with_capacity(new_cap);

        // Copy the bytes
        v.extend_from_slice(self.as_ref());

        let ptr = vptr(v.as_mut_ptr());
        let cap = v.capacity();
        self.data = Arc::new(v);

        // Update self
        self.ptr = ptr;
        self.cap = cap;
        debug_assert_eq!(self.len, self.data.len());
        true
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

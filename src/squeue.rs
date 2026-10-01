//! Submission Queue

use std::fmt::{self, Debug, Display, Formatter};
use std::{cell::Cell, error::Error, mem, sync::atomic};

use bitflags::bitflags;

use crate::{sys, util::private, util::unsync_load, util::Mmap};

pub(crate) struct Inner<E: EntryMarker> {
    pub(crate) head: *const atomic::AtomicU32,
    pub(crate) tail: *const atomic::AtomicU32,
    pub(crate) flags: *const atomic::AtomicU32,
    pub(crate) ring_mask: u32,
    pub(crate) ring_entries: usize,
    dropped: *const atomic::AtomicU32,

    pub(crate) sqes: *mut E,

    pub(crate) local_head: Cell<u32>,
    pub(crate) local_tail: Cell<u32>,

    /// Set while a `push_inline` closure runs, to detect re-entrant pushes.
    #[cfg(debug_assertions)]
    pushing: Cell<bool>,
}

#[derive(Clone)]
/// An io_uring instance's submission queue. This is used to send I/O requests to the kernel.
pub struct SubmissionQueue<'a, E: EntryMarker = Entry> {
    pub(crate) queue: &'a Inner<E>,
}

impl<'a, E: EntryMarker> Copy for SubmissionQueue<'a, E> {}

/// A submission queue entry (SQE), representing a request for an I/O operation.
///
/// This is implemented for [`Entry`] and [`Entry128`].
pub trait EntryMarker: Clone + Debug + From<Entry> + private::Sealed {
    const BUILD_FLAGS: u32;

    /// Set the application-supplied user data.
    fn set_user_data(&mut self, user_data: u64);

    /// Get the application-supplied user data.
    fn get_user_data(&self) -> u64;
}

/// A 64-byte submission queue entry (SQE), representing a request for an I/O operation.
///
/// These can be created via opcodes in [`opcode`](crate::opcode).
///
/// # Example
///
/// ```
/// use io_uring::{opcode, types};
/// use std::ffi::CString;
///
/// let path = CString::new("/etc/passwd").unwrap();
///
/// // Create an Entry to open /etc/passwd for reading
/// let entry = opcode::OpenAt::new(types::Fd(libc::AT_FDCWD), path.as_ptr())
///     .flags(libc::O_RDONLY)
///     .build()
///     .user_data(0x42);
/// ```
#[repr(C)]
pub struct Entry(pub(crate) sys::io_uring_sqe);

/// A 128-byte submission queue entry (SQE), representing a request for an I/O operation.
///
/// These can be created via opcodes in [`opcode`](crate::opcode), or by converting
/// from an [`Entry`] using the [`From`] trait.
///
/// # Example
///
/// ```
/// use io_uring::{opcode, squeue::Entry128, types};
/// use std::ffi::CString;
///
/// let path = CString::new("/etc/passwd").unwrap();
///
/// // Create an Entry128 to open /etc/passwd for reading
/// let entry = Entry128::from(
///     opcode::OpenAt::new(types::Fd(libc::AT_FDCWD), path.as_ptr())
///         .flags(libc::O_RDONLY)
///         .build()
///         .user_data(0x42)
/// );
/// ```
#[repr(C)]
#[derive(Clone)]
pub struct Entry128(pub(crate) Entry, pub(crate) [u8; 64]);

#[test]
fn test_entry_sizes() {
    assert_eq!(mem::size_of::<Entry>(), 64);
    assert_eq!(mem::size_of::<Entry128>(), 128);
}

#[test]
fn test_user_data() {
    fn replace<E: EntryMarker>(entry: &mut E, user_data: u64) -> u64 {
        let previous = entry.get_user_data();
        entry.set_user_data(user_data);
        previous
    }

    let mut entry = crate::opcode::Nop::new().build().user_data(1);
    assert_eq!(replace(&mut entry, 2), 1);
    assert_eq!(entry.get_user_data(), 2);

    let mut entry = Entry128::from(crate::opcode::Nop::new().build().user_data(3));
    assert_eq!(replace(&mut entry, 4), 3);
    assert_eq!(entry.get_user_data(), 4);
}

bitflags! {
    /// Submission flags
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Flags: u8 {
        /// When this flag is specified,
        /// `fd` is an index into the files array registered with the io_uring instance.
        #[doc(hidden)]
        const FIXED_FILE = 1 << sys::IOSQE_FIXED_FILE_BIT;

        /// When this flag is specified,
        /// the SQE will not be started before previously submitted SQEs have completed,
        /// and new SQEs will not be started before this one completes.
        const IO_DRAIN = 1 << sys::IOSQE_IO_DRAIN_BIT;

        /// When this flag is specified,
        /// it forms a link with the next SQE in the submission ring.
        /// That next SQE will not be started before this one completes.
        const IO_LINK = 1 << sys::IOSQE_IO_LINK_BIT;

        /// Like [`IO_LINK`](Self::IO_LINK), but it doesn’t sever regardless of the completion
        /// result.
        const IO_HARDLINK = 1 << sys::IOSQE_IO_HARDLINK_BIT;

        /// Normal operation for io_uring is to try and issue an sqe as non-blocking first,
        /// and if that fails, execute it in an async manner.
        ///
        /// To support more efficient overlapped operation of requests
        /// that the application knows/assumes will always (or most of the time) block,
        /// the application can ask for an sqe to be issued async from the start.
        const ASYNC = 1 << sys::IOSQE_ASYNC_BIT;

        /// Conceptually the kernel holds a set of buffers organized into groups. When you issue a
        /// request with this flag and set `buf_group` to a valid buffer group ID (e.g.
        /// [`buf_group` on `Read`](crate::opcode::Read::buf_group)) then once the file descriptor
        /// becomes ready the kernel will try to take a buffer from the group.
        ///
        /// If there are no buffers in the group, your request will fail with `-ENOBUFS`. Otherwise,
        /// the corresponding [`cqueue::Entry::flags`](crate::cqueue::Entry::flags) will contain the
        /// chosen buffer ID, encoded with:
        ///
        /// ```text
        /// (buffer_id << IORING_CQE_BUFFER_SHIFT) | IORING_CQE_F_BUFFER
        /// ```
        ///
        /// You can use [`buffer_select`](crate::cqueue::buffer_select) to take the buffer ID.
        ///
        /// The buffer will then be removed from the group and won't be usable by other requests
        /// anymore.
        ///
        /// You can provide new buffers in a group with
        /// [`ProvideBuffers`](crate::opcode::ProvideBuffers).
        ///
        /// See also [the LWN thread on automatic buffer
        /// selection](https://lwn.net/Articles/815491/).
        const BUFFER_SELECT = 1 << sys::IOSQE_BUFFER_SELECT_BIT;

        /// Don't post CQE if request succeeded.
        const SKIP_SUCCESS = 1 << sys::IOSQE_CQE_SKIP_SUCCESS_BIT;
    }
}

impl<E: EntryMarker> Inner<E> {
    #[rustfmt::skip]
    pub(crate) unsafe fn new(
        sq_mmap: &Mmap,
        sqe_mmap: &Mmap,
        p: &sys::io_uring_params,
    ) -> Self {
        let head         = sq_mmap.offset(p.sq_off.head        ) as *const atomic::AtomicU32;
        let tail         = sq_mmap.offset(p.sq_off.tail        ) as *const atomic::AtomicU32;
        let ring_mask    = sq_mmap.offset(p.sq_off.ring_mask   ).cast::<u32>().read();
        let ring_entries = sq_mmap.offset(p.sq_off.ring_entries).cast::<u32>().read();
        let flags        = sq_mmap.offset(p.sq_off.flags       ) as *const atomic::AtomicU32;
        let dropped      = sq_mmap.offset(p.sq_off.dropped     ) as *const atomic::AtomicU32;
        let sqes         = sqe_mmap.as_mut_ptr() as *mut E;

        // Initialize the SQ array with an identity mapping unless NO_SQARRAY is set, in which case
        // the kernel consumes SQEs directly by ring index and no array exists.
        if p.flags & sys::IORING_SETUP_NO_SQARRAY == 0 {
            let array = sq_mmap.offset(p.sq_off.array) as *mut u32;
            for i in 0..ring_entries {
                array.add(i as usize).write_volatile(i);
            }
        }

        let ring_entries = ring_entries as usize;
        let local_head = Cell::new((*head).load(atomic::Ordering::Acquire));
        let local_tail = Cell::new(unsync_load(tail));

        Self {
            head,
            tail,
            ring_mask,
            ring_entries,
            flags,
            dropped,
            sqes,
            local_head,
            local_tail,
            #[cfg(debug_assertions)]
            pushing: Cell::new(false),
        }
    }

    #[inline]
    pub(crate) fn borrow(&self) -> SubmissionQueue<'_, E> {
        SubmissionQueue { queue: self }
    }

    #[inline]
    pub(crate) fn sync(&self) {
        unsafe {
            publish_tail(self.tail, self.local_tail.get());
            self.local_head
                .set((*self.head).load(atomic::Ordering::Acquire));
        }
    }

    #[cold]
    #[inline(never)]
    fn sync_cold(&self) {
        self.sync();
    }

    #[inline]
    #[track_caller]
    fn debug_assert_not_pushing(&self) {
        #[cfg(debug_assertions)]
        assert!(
            !self.pushing.get(),
            "submission queue push from within a `push_inline` closure"
        );
    }
}

/// Marks the queue as inside a `push_inline` closure; cleared on drop, including on unwind.
#[cfg(debug_assertions)]
struct PushGuard<'a>(&'a Cell<bool>);

#[cfg(debug_assertions)]
impl Drop for PushGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Publish the local tail to the kernel.
///
/// The tail is only written by userspace, so skip the store if it is already published,
/// to avoid dirtying a cache line shared with the kernel (sq poll thread).
#[inline]
pub(crate) unsafe fn publish_tail(tail: *const atomic::AtomicU32, local_tail: u32) {
    if unsync_load(tail) != local_tail {
        (*tail).store(local_tail, atomic::Ordering::Release);
    }
}

impl<E: EntryMarker> SubmissionQueue<'_, E> {
    /// Synchronize this type with the real submission queue.
    ///
    /// This will flush any entries added by [`push`](Self::push) or
    /// [`push_multiple`](Self::push_multiple) and will update the queue's length if the kernel has
    /// consumed some entries in the meantime.
    #[inline]
    pub fn sync(&self) {
        self.queue.sync();
    }

    /// When [`is_setup_sqpoll`](crate::Parameters::is_setup_sqpoll) is set, whether the kernel
    /// threads has gone to sleep and requires a system call to wake it up.
    ///
    /// A result of `false` is only meaningful if the function was called after the latest update
    /// to the queue head. Other interpretations could lead to a race condition where the kernel
    /// concurrently put the device to sleep and no further progress is made.
    #[inline]
    pub fn need_wakeup(&self) -> bool {
        // See discussions that happened in [#197] and its linked threads in liburing. We need to
        // ensure that writes to the head have been visible _to the kernel_ if this load results in
        // decision to sleep. This is solved with a SeqCst fence. There is no common modified
        // memory location that would provide alternative synchronization.
        //
        // The kernel, from its sequencing, first writes the wake flag, then performs a full
        // barrier (`smp_mb`, or `smp_mb__after_atomic`), then reads the head. We assume that our
        // user first writes the head and then reads the `need_wakeup` flag as documented. It is
        // necessary to ensure that at least one observes the other write. By establishing a point
        // of sequential consistency on both sides between their respective write and read, at
        // least one coherency order holds. With regards to the interpretation of the atomic memory
        // model of Rust (that is, that of C++20) we're assuming that an `smp_mb` provides at least
        // the effect of a `fence(SeqCst)`.
        //
        // [#197]: https://github.com/tokio-rs/io-uring/issues/197
        atomic::fence(atomic::Ordering::SeqCst);
        unsafe {
            (*self.queue.flags).load(atomic::Ordering::Relaxed) & sys::IORING_SQ_NEED_WAKEUP != 0
        }
    }

    /// The effect of [`Self::need_wakeup`], after synchronization work performed by the caller.
    ///
    /// This function should only be called if the caller can guarantee that a `SeqCst` fence has
    /// been inserted after the last write to the queue's head. The function is then a little more
    /// efficient by avoiding to perform one itself.
    ///
    /// Failure to uphold the precondition can result in an effective dead-lock due to a sleeping
    /// device.
    #[inline]
    pub fn need_wakeup_after_intermittent_seqcst(&self) -> bool {
        unsafe {
            (*self.queue.flags).load(atomic::Ordering::Relaxed) & sys::IORING_SQ_NEED_WAKEUP != 0
        }
    }

    /// The number of invalid submission queue entries that have been encountered in the ring
    /// buffer.
    pub fn dropped(&self) -> u32 {
        unsafe { (*self.queue.dropped).load(atomic::Ordering::Acquire) }
    }

    /// Returns `true` if the completion queue ring is overflown.
    pub fn cq_overflow(&self) -> bool {
        unsafe {
            (*self.queue.flags).load(atomic::Ordering::Acquire) & sys::IORING_SQ_CQ_OVERFLOW != 0
        }
    }

    /// Returns `true` if completions are pending that should be processed. Only relevant when used
    /// in conjuction with the `setup_taskrun_flag` function. Available since 5.19.
    pub fn taskrun(&self) -> bool {
        unsafe { (*self.queue.flags).load(atomic::Ordering::Acquire) & sys::IORING_SQ_TASKRUN != 0 }
    }

    /// Get the total number of entries in the submission queue ring buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.queue.ring_entries
    }

    /// Get the number of submission queue events in the ring buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.queue
            .local_tail
            .get()
            .wrapping_sub(self.queue.local_head.get()) as usize
    }

    /// Returns `true` if the submission queue ring buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the submission queue ring buffer has reached capacity, and no more events
    /// can be added before the kernel consumes some.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    /// Check that `n` entries can be pushed, synchronizing with the kernel if the queue looks full.
    #[inline]
    fn has_space(&self, n: usize) -> bool {
        if self.capacity() - self.len() >= n {
            true
        } else {
            self.queue.sync_cold();
            self.capacity() - self.len() >= n
        }
    }

    /// Attempts to push an entry into the queue, initializing it in place with `f`.
    /// The entry passed to `f` is zeroed.
    /// If the queue is full, an error is returned and `f` is not called.
    ///
    /// # Safety
    ///
    /// Developers must ensure that parameters of the entry (such as buffer) are valid and will
    /// be valid for the entire duration of the operation, otherwise it may cause memory problems.
    ///
    /// `f` must not push to this submission queue (via [`push`](Self::push),
    /// [`push_inline`](Self::push_inline) or [`push_multiple`](Self::push_multiple)). The entry
    /// is not reserved until `f` returns, so a nested push would alias the entry being
    /// initialized and could submit a stale entry or overrun the queue. Debug builds panic on
    /// such a nested push.
    #[inline]
    #[track_caller]
    pub unsafe fn push_inline<F>(&self, f: F) -> Result<(), PushError>
    where
        F: FnOnce(&mut E),
    {
        self.queue.debug_assert_not_pushing();
        if self.has_space(1) {
            #[cfg(debug_assertions)]
            let _guard = {
                self.queue.pushing.set(true);
                PushGuard(&self.queue.pushing)
            };

            let entry = self
                .queue
                .sqes
                .add((self.queue.local_tail.get() & self.queue.ring_mask) as usize);
            // slot may hold an entry already consumed by the kernel
            entry.write_bytes(0, 1);
            f(&mut *entry);
            self.queue
                .local_tail
                .set(self.queue.local_tail.get().wrapping_add(1));
            Ok(())
        } else {
            Err(PushError)
        }
    }

    /// Attempts to push an entry into the queue.
    /// If the queue is full, an error is returned.
    ///
    /// # Safety
    ///
    /// Developers must ensure that parameters of the entry (such as buffer) are valid and will
    /// be valid for the entire duration of the operation, otherwise it may cause memory problems.
    #[inline]
    #[track_caller]
    pub unsafe fn push(&self, entry: &E) -> Result<(), PushError> {
        self.queue.debug_assert_not_pushing();
        if self.has_space(1) {
            self.push_unchecked(entry);
            Ok(())
        } else {
            Err(PushError)
        }
    }

    /// Attempts to push several entries into the queue.
    /// If the queue does not have space for all of the entries, an error is returned.
    ///
    /// # Safety
    ///
    /// Developers must ensure that parameters of all the entries (such as buffer) are valid and
    /// will be valid for the entire duration of the operation, otherwise it may cause memory
    /// problems.
    #[inline]
    #[track_caller]
    pub unsafe fn push_multiple(&self, entries: &[E]) -> Result<(), PushError> {
        self.queue.debug_assert_not_pushing();
        if !self.has_space(entries.len()) {
            return Err(PushError);
        }

        let mut tail = self.queue.local_tail.get();
        for entry in entries {
            *self.queue.sqes.add((tail & self.queue.ring_mask) as usize) = entry.clone();
            tail = tail.wrapping_add(1);
        }
        self.queue.local_tail.set(tail);

        Ok(())
    }

    #[inline]
    unsafe fn push_unchecked(&self, entry: &E) {
        *self
            .queue
            .sqes
            .add((self.queue.local_tail.get() & self.queue.ring_mask) as usize) = entry.clone();
        self.queue
            .local_tail
            .set(self.queue.local_tail.get().wrapping_add(1));
    }
}

impl Entry {
    /// Set the submission event's [flags](Flags).
    #[inline]
    pub fn flags(mut self, flags: Flags) -> Entry {
        self.0.flags |= flags.bits();
        self
    }

    /// Clear the submission event's [flags](Flags).
    #[inline]
    pub fn clear_flags(mut self) -> Entry {
        self.0.flags = 0;
        self
    }

    /// Set the user data. This is an application-supplied value that will be passed straight
    /// through into the [completion queue entry](crate::cqueue::Entry::user_data).
    #[inline]
    pub fn user_data(mut self, user_data: u64) -> Entry {
        self.0.user_data = user_data;
        self
    }

    /// Set the user_data without consuming the entry.
    #[inline]
    pub fn set_user_data(&mut self, user_data: u64) {
        self.0.user_data = user_data;
    }

    /// Get the previously application-supplied user data.
    #[inline]
    pub fn get_user_data(&self) -> u64 {
        self.0.user_data
    }

    /// Get the opcode associated with this entry.
    #[inline]
    pub fn get_opcode(&self) -> u32 {
        self.0.opcode.into()
    }

    /// Set the personality of this event. You can obtain a personality using
    /// [`Submitter::register_personality`](crate::Submitter::register_personality).
    pub fn personality(mut self, personality: u16) -> Entry {
        self.0.personality = personality;
        self
    }
}

impl Default for Entry {
    fn default() -> Self {
        Self(unsafe { mem::zeroed() })
    }
}

impl private::Sealed for Entry {}

impl EntryMarker for Entry {
    const BUILD_FLAGS: u32 = 0;

    #[inline]
    fn set_user_data(&mut self, user_data: u64) {
        Entry::set_user_data(self, user_data);
    }

    #[inline]
    fn get_user_data(&self) -> u64 {
        Entry::get_user_data(self)
    }
}

impl Clone for Entry {
    #[inline(always)]
    fn clone(&self) -> Entry {
        // io_uring_sqe doesn't implement Clone due to the 'cmd' incomplete array field.
        Entry(unsafe { mem::transmute_copy(&self.0) })
    }
}

impl Debug for Entry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("op_code", &self.0.opcode)
            .field("flags", &self.0.flags)
            .field("user_data", &self.0.user_data)
            .finish()
    }
}

impl Entry128 {
    /// Set the submission event's [flags](Flags).
    #[inline]
    pub fn flags(mut self, flags: Flags) -> Entry128 {
        self.0 .0.flags |= flags.bits();
        self
    }

    /// Clear the submission event's [flags](Flags).
    #[inline]
    pub fn clear_flags(mut self) -> Entry128 {
        self.0 .0.flags = 0;
        self
    }

    /// Set the user data. This is an application-supplied value that will be passed straight
    /// through into the [completion queue entry](crate::cqueue::Entry::user_data).
    #[inline]
    pub fn user_data(mut self, user_data: u64) -> Entry128 {
        self.0 .0.user_data = user_data;
        self
    }

    /// Set the user data without consuming the entry.
    #[inline]
    pub fn set_user_data(&mut self, user_data: u64) {
        self.0 .0.user_data = user_data;
    }

    /// Get the previously application-supplied user data.
    #[inline]
    pub fn get_user_data(&self) -> u64 {
        self.0 .0.user_data
    }

    /// Set the personality of this event. You can obtain a personality using
    /// [`Submitter::register_personality`](crate::Submitter::register_personality).
    #[inline]
    pub fn personality(mut self, personality: u16) -> Entry128 {
        self.0 .0.personality = personality;
        self
    }

    /// Get the opcode associated with this entry.
    #[inline]
    pub fn get_opcode(&self) -> u32 {
        self.0 .0.opcode.into()
    }
}

impl private::Sealed for Entry128 {}

impl EntryMarker for Entry128 {
    const BUILD_FLAGS: u32 = sys::IORING_SETUP_SQE128;

    #[inline]
    fn set_user_data(&mut self, user_data: u64) {
        Entry128::set_user_data(self, user_data);
    }

    #[inline]
    fn get_user_data(&self) -> u64 {
        Entry128::get_user_data(self)
    }
}

impl From<Entry> for Entry128 {
    fn from(entry: Entry) -> Entry128 {
        Entry128(entry, [0u8; 64])
    }
}

impl Debug for Entry128 {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry128")
            .field("op_code", &self.0 .0.opcode)
            .field("flags", &self.0 .0.flags)
            .field("user_data", &self.0 .0.user_data)
            .finish()
    }
}

/// An error pushing to the submission queue due to it being full.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PushError;

impl Display for PushError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("submission queue is full")
    }
}

impl Error for PushError {}

impl<E: EntryMarker> Debug for SubmissionQueue<'_, E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_list();
        let mut pos = self.queue.local_head.get();
        while pos != self.queue.local_tail.get() {
            let entry: &E = unsafe { &*self.queue.sqes.add((pos & self.queue.ring_mask) as usize) };
            d.entry(&entry);
            pos = pos.wrapping_add(1);
        }
        d.finish()
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use crate::{opcode, IoUring};

    #[test]
    #[should_panic(expected = "push from within a `push_inline` closure")]
    fn push_inline_reentrant_push_panics() {
        let ring = IoUring::new(4).unwrap();
        let sq = ring.submission();
        unsafe {
            let _ = sq.push_inline(|e| {
                *e = opcode::Nop::new().build();
                let _ = sq.push(&opcode::Nop::new().build());
            });
        }
    }

    #[test]
    fn push_inline_guard_reset_after_panic() {
        let ring = IoUring::new(4).unwrap();
        let sq = ring.submission();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let _ = sq.push_inline(|_| panic!("boom"));
        }));
        assert!(res.is_err());
        unsafe {
            sq.push_inline(|e| *e = opcode::Nop::new().build()).unwrap();
            sq.push(&opcode::Nop::new().build()).unwrap();
        }
    }
}

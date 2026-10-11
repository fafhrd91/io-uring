//! Operation codes that initialize [`squeue::Entry`](crate::squeue::Entry)s in place.
//!
//! Each operation is constructed with `new`, which takes the entry to initialize along with
//! the operation's required arguments. Optional parameters are written by the setters.
//!
//! The entry must be zeroed before `new` is called, entries provided by
//! [`SubmissionQueue::push`](crate::SubmissionQueue::push) and [`Entry::default`] are.
//!
//! ```
//! use ntex_io_uring::{opcode, squeue::Entry, types};
//! use std::ffi::CString;
//!
//! let path = CString::new("/etc/passwd").unwrap();
//!
//! let mut entry = Entry::default();
//! opcode::OpenAt::new(&mut entry, types::Fd(libc::AT_FDCWD), path.as_ptr())
//!     .flags(libc::O_RDONLY)
//!     .user_data(0x42);
//! ```

#![allow(clippy::new_ret_no_self, clippy::too_many_arguments)]

use std::os::unix::io::RawFd;
use std::{convert::TryInto, mem};

use crate::squeue::{Entry, Entry128, Flags};
use crate::sys;
use crate::types::{self, sealed};

macro_rules! assign_fd {
    ( $sqe:ident, $fd:expr ) => {
        match sealed::UseFixed::into($fd) {
            sealed::Target::Fd(fd) => $sqe.fd = fd,
            sealed::Target::Fixed(idx) => {
                $sqe.fd = idx as _;
                $sqe.flags |= Flags::FIXED_FILE.bits();
            }
        }
    };
}

macro_rules! opcode_common {
    () => {
        /// Set the user data. This is an application-supplied value that will be passed
        /// straight through into the [completion queue entry](crate::cqueue::Entry::user_data).
        #[inline]
        pub fn user_data(self, user_data: u64) -> Self {
            self.sqe.user_data = user_data;
            self
        }

        /// Set the submission event's [flags](Flags), they are ORed with already set flags.
        #[inline]
        pub fn sqe_flags(self, flags: Flags) -> Self {
            self.sqe.flags |= flags.bits();
            self
        }

        /// Set the personality of this event. You can obtain a personality using
        /// [`Submitter::register_personality`](crate::Submitter::register_personality).
        #[inline]
        pub fn personality(self, personality: u16) -> Self {
            self.sqe.personality = personality;
            self
        }
    };
}

macro_rules! opcode {
    (
        $( #[$outer:meta] )*
        pub struct $name:ident = $code:expr;

        $( #[$new_meta:meta] )*
        fn new($sqe:ident $(, $arg:ident : $aty:ty )* $(,)?) $new_body:block

        $(
            $( #[$set_meta:meta] )*
            fn $setter:ident($s:ident $(, $sarg:ident : $saty:ty )* $(,)?) $set_body:block
        )*
    ) => {
        $( #[$outer] )*
        pub struct $name<'a> {
            sqe: &'a mut sys::io_uring_sqe,
        }

        impl<'a> $name<'a> {
            /// The opcode of the operation. This can be passed to
            /// [`Probe::is_supported`](crate::Probe::is_supported) to check if this operation is
            /// supported with the current kernel.
            pub const CODE: u8 = $code as _;

            $( #[$new_meta] )*
            #[inline]
            pub fn new(entry: &'a mut impl AsMut<Entry> $(, $arg: $aty )*) -> Self {
                let $sqe = &mut entry.as_mut().0;
                $sqe.opcode = Self::CODE;
                $new_body
                Self { sqe: $sqe }
            }

            $(
                $( #[$set_meta] )*
                #[inline]
                pub fn $setter(self $(, $sarg: $saty )*) -> Self {
                    {
                        let $s = &mut *self.sqe;
                        $set_body
                    }
                    self
                }
            )*

            opcode_common!();
        }
    };
}

opcode! {
    /// Do not perform any I/O.
    ///
    /// This is useful for testing the performance of the io_uring implementation itself.
    pub struct Nop = sys::IORING_OP_NOP;

    fn new(sqe) {
        sqe.fd = -1;
    }
}

opcode! {
    /// Vectored read, equivalent to `preadv2(2)`.
    pub struct Readv = sys::IORING_OP_READV;

    fn new(sqe, fd: impl sealed::UseFixed, iovec: *const libc::iovec, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = iovec as _;
        sqe.len = len;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// specified for read operations, contains a bitwise OR of per-I/O flags,
    /// as described in the `preadv2(2)` man page.
    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    fn buf_group(sqe, buf_group: u16) {
        sqe.__bindgen_anon_4.buf_group = buf_group;
    }
}

opcode! {
    /// Vectored write, equivalent to `pwritev2(2)`.
    pub struct Writev = sys::IORING_OP_WRITEV;

    fn new(sqe, fd: impl sealed::UseFixed, iovec: *const libc::iovec, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = iovec as _;
        sqe.len = len;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    /// File offset to write at, defaults to `0`.
    ///
    /// Use `u64::MAX` to write at the current file position.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// specified for write operations, contains a bitwise OR of per-I/O flags,
    /// as described in the `preadv2(2)` man page.
    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    /// Write stream id (Linux 6.16+); `0` means no stream.
    /// Older kernels ignore this field.
    fn write_stream(sqe, write_stream: u8) {
        sqe.__bindgen_anon_5.__bindgen_anon_2.write_stream = write_stream;
    }
}

opcode! {
    /// File sync. See also `fsync(2)`.
    ///
    /// Optionally [`offset`](Self::offset) and [`len`](Self::len) can be used to specify a range
    /// within the file to be synced rather than syncing the entire file, which is the default
    /// behavior.
    ///
    /// Note that, while I/O is initiated in the order in which it appears in the submission queue,
    /// completions are unordered. For example, an application which places a write I/O followed by
    /// an fsync in the submission queue cannot expect the fsync to apply to the write. The two
    /// operations execute in parallel, so the fsync may complete before the write is issued to the
    /// storage. The same is also true for previously issued writes that have not completed prior to
    /// the fsync. To enforce ordering one may utilize linked SQEs, `IOSQE_IO_DRAIN` or wait for the
    /// arrival of CQEs of requests which have to be ordered before a given request before
    /// submitting its SQE.
    pub struct Fsync = sys::IORING_OP_FSYNC;

    fn new(sqe, fd: impl sealed::UseFixed) {
        assign_fd!(sqe, fd);
    }

    /// Offset within the file to start the sync range. Together with [`len`](Self::len), this
    /// selects a byte range; when both are zero (the default), the entire file is synced.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// Length of the range to sync, in bytes. When zero (the default) with
    /// [`offset`](Self::offset) also zero, the entire file is synced.
    fn len(sqe, len: u32) {
        sqe.len = len;
    }

    /// The `flags` bit mask may contain either 0, for a normal file integrity sync,
    /// or [types::FsyncFlags::DATASYNC] to provide data sync only semantics.
    /// See the descriptions of `O_SYNC` and `O_DSYNC` in the `open(2)` manual page for more information.
    fn flags(sqe, flags: types::FsyncFlags) {
        sqe.__bindgen_anon_3.fsync_flags = flags.bits();
    }
}

opcode! {
    /// Read from a file into a fixed buffer that has been previously registered with
    /// [`Submitter::register_buffers`](crate::Submitter::register_buffers).
    ///
    /// The return values match those documented in the `preadv2(2)` man pages.
    pub struct ReadFixed = sys::IORING_OP_READ_FIXED;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *mut u8, len: u32, buf_index: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
        sqe.__bindgen_anon_4.buf_index = buf_index;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    /// The offset of the file to read from.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// Specified for read operations, contains a bitwise OR of per-I/O flags, as described in
    /// the `preadv2(2)` man page.
    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }
}

opcode! {
    /// Write to a file from a fixed buffer that have been previously registered with
    /// [`Submitter::register_buffers`](crate::Submitter::register_buffers).
    ///
    /// The return values match those documented in the `pwritev2(2)` man pages.
    pub struct WriteFixed = sys::IORING_OP_WRITE_FIXED;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *const u8, len: u32, buf_index: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
        sqe.__bindgen_anon_4.buf_index = buf_index;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    /// The offset of the file to write to.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// Specified for write operations, contains a bitwise OR of per-I/O flags, as described in
    /// the `pwritev2(2)` man page.
    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    /// Write stream id (Linux 6.16+); `0` means no stream.
    /// Older kernels ignore this field.
    fn write_stream(sqe, write_stream: u8) {
        sqe.__bindgen_anon_5.__bindgen_anon_2.write_stream = write_stream;
    }
}

opcode! {
    /// Poll the specified fd.
    ///
    /// Unlike poll or epoll without `EPOLLONESHOT`, this interface defaults to work in one shot mode.
    /// That is, once the poll operation is completed, it will have to be resubmitted.
    ///
    /// If multi is set, the poll will work in multi shot mode instead. That means it will
    /// repeatedly trigger when the requested event becomes true, and hence multiple CQEs can be
    /// generated from this single submission. The CQE flags field will have IORING_CQE_F_MORE set
    /// on completion if the application should expect further CQE entries from the original
    /// request. If this flag isn't set on completion, then the poll request has been terminated
    /// and no further events will be generated. This mode is available since 5.13.
    pub struct PollAdd = sys::IORING_OP_POLL_ADD;

    /// The bits that may be set in `flags` are defined in `<poll.h>`,
    /// and documented in `poll(2)`.
    fn new(sqe, fd: impl sealed::UseFixed, flags: u32) {
        assign_fd!(sqe, fd);

        #[cfg(target_endian = "little")]
        {
            sqe.__bindgen_anon_3.poll32_events = flags;
        }

        #[cfg(target_endian = "big")]
        {
            sqe.__bindgen_anon_3.poll32_events = (flags << 16) | (flags >> 16);
        }
    }

    fn multi(sqe, multi: bool) {
        sqe.len = if multi { sys::IORING_POLL_ADD_MULTI } else { 0 };
    }
}

opcode! {
    /// Remove an existing [poll](PollAdd) request.
    ///
    /// If found, the `result` method of the `cqueue::Entry` will return 0.
    /// If not found, `result` will return `-libc::ENOENT`.
    pub struct PollRemove = sys::IORING_OP_POLL_REMOVE;

    fn new(sqe, user_data: u64) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = user_data;
    }
}

opcode! {
    /// Sync a file segment with disk, equivalent to `sync_file_range(2)`.
    pub struct SyncFileRange = sys::IORING_OP_SYNC_FILE_RANGE;

    fn new(sqe, fd: impl sealed::UseFixed, len: u32) {
        assign_fd!(sqe, fd);
        sqe.len = len;
    }

    /// the offset method holds the offset in bytes
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// the flags method holds the flags for the command
    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.sync_range_flags = flags;
    }
}

opcode! {
    /// Send a message on a socket, equivalent to `sendmsg(2)`.
    ///
    /// fd must be set to the socket file descriptor, msg must contain a pointer to the msghdr
    /// structure, and flags holds the flags associated with the system call.
    /// The message header must stay valid until the operation completes.
    pub struct SendMsg = sys::IORING_OP_SENDMSG;

    fn new(sqe, fd: impl sealed::UseFixed, msg: *const libc::msghdr) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = msg as _;
        sqe.len = 1;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.msg_flags = flags;
    }
}

opcode! {
    /// Receive a message on a socket, equivalent to `recvmsg(2)`.
    ///
    /// See also the description of [`SendMsg`].
    pub struct RecvMsg = sys::IORING_OP_RECVMSG;

    fn new(sqe, fd: impl sealed::UseFixed, msg: *mut libc::msghdr) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = msg as _;
        sqe.len = 1;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.msg_flags = flags;
    }

    fn buf_group(sqe, buf_group: u16) {
        sqe.__bindgen_anon_4.buf_group = buf_group;
    }
}

opcode! {
    /// Receive multiple messages on a socket, equivalent to `recvmsg(2)`.
    ///
    /// Parameters:
    ///     msg:       For this multishot variant of ResvMsg, only the msg_namelen and msg_controllen
    ///                fields are relevant.
    ///     buf_group: The id of the provided buffer pool to use for each received message.
    ///
    /// See also the description of [`SendMsg`] and [`types::RecvMsgOut`].
    ///
    /// The multishot version allows the application to issue a single receive request, which
    /// repeatedly posts a CQE when data is available. It requires the MSG_WAITALL flag is not set.
    /// Each CQE will take a buffer out of a provided buffer pool for receiving. The application
    /// should check the flags of each CQE, regardless of its result. If a posted CQE does not have
    /// the IORING_CQE_F_MORE flag set then the multishot receive will be done and the application
    /// should issue a new request.
    ///
    /// Unlike [`RecvMsg`], this multishot recvmsg will prepend a struct which describes the layout
    /// of the rest of the buffer in combination with the initial msghdr structure submitted with
    /// the request. Use [`types::RecvMsgOut`] to parse the data received and access its
    /// components.
    ///
    /// The recvmsg multishot variant is available since kernel 6.0.
    pub struct RecvMsgMulti = sys::IORING_OP_RECVMSG;

    fn new(sqe, fd: impl sealed::UseFixed, msg: *const libc::msghdr, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = msg as _;
        sqe.len = 1;
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
        sqe.ioprio = sys::IORING_RECV_MULTISHOT as _;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.msg_flags = flags;
    }
}

opcode! {
    /// Register a timeout operation.
    ///
    /// A timeout will trigger a wakeup event on the completion ring for anyone waiting for events.
    /// A timeout condition is met when either the specified timeout expires, or the specified number of events have completed.
    /// Either condition will trigger the event.
    /// The request will complete with `-ETIME` if the timeout got completed through expiration of the timer,
    /// or 0 if the timeout got completed through requests completing on their own.
    /// If the timeout was cancelled before it expired, the request will complete with `-ECANCELED`.
    pub struct Timeout = sys::IORING_OP_TIMEOUT;

    fn new(sqe, timespec: *const types::Timespec) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = timespec as _;
        sqe.len = 1;
    }

    /// `count` may contain a completion event count.
    /// If [`TimeoutFlags::MULTISHOT`](types::TimeoutFlags::MULTISHOT) is set in `flags`, this is the number of repeats.
    /// A value of 0 means the timeout is indefinite and can only be stopped by a removal request.
    fn count(sqe, count: u32) {
        sqe.__bindgen_anon_1.off = count as _;
    }

    fn flags(sqe, flags: types::TimeoutFlags) {
        sqe.__bindgen_anon_3.timeout_flags = flags.bits();
    }
}

// === 5.5 ===

opcode! {
    /// Attempt to remove an existing [timeout operation](Timeout).
    pub struct TimeoutRemove = sys::IORING_OP_TIMEOUT_REMOVE;

    fn new(sqe, user_data: u64) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = user_data;
    }
}

opcode! {
    /// Attempt to update an existing [timeout operation](Timeout) with a new timespec.
    /// The optional `count` value of the original timeout value cannot be updated.
    pub struct TimeoutUpdate = sys::IORING_OP_TIMEOUT_REMOVE;

    fn new(sqe, user_data: u64, timespec: *const types::Timespec) {
        sqe.fd = -1;
        sqe.__bindgen_anon_1.off = timespec as _;
        sqe.__bindgen_anon_2.addr = user_data;
        sqe.__bindgen_anon_3.timeout_flags = sys::IORING_TIMEOUT_UPDATE;
    }

    fn flags(sqe, flags: types::TimeoutFlags) {
        sqe.__bindgen_anon_3.timeout_flags = flags.bits() | sys::IORING_TIMEOUT_UPDATE;
    }
}

opcode! {
    /// Accept a new connection on a socket, equivalent to `accept4(2)`.
    pub struct Accept = sys::IORING_OP_ACCEPT;

    fn new(sqe, fd: impl sealed::UseFixed, addr: *mut libc::sockaddr, addrlen: *mut libc::socklen_t) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = addr as _;
        sqe.__bindgen_anon_1.addr2 = addrlen as _;
    }

    fn file_index(sqe, file_index: Option<types::DestinationSlot>) {
        sqe.__bindgen_anon_5.file_index = file_index.map_or(0, |d| d.kernel_index_arg());
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.accept_flags = flags as _;
    }
}

opcode! {
    /// Set a socket option.
    pub struct SetSockOpt = sys::IORING_OP_URING_CMD;

    fn new(
        sqe,
        fd: impl sealed::UseFixed,
        level: u32,
        optname: u32,
        optval: *const libc::c_void,
        optlen: u32,
    ) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_1.__bindgen_anon_1.cmd_op = sys::SOCKET_URING_OP_SETSOCKOPT;
        sqe.__bindgen_anon_2.__bindgen_anon_1.level = level;
        sqe.__bindgen_anon_2.__bindgen_anon_1.optname = optname;
        sqe.__bindgen_anon_5.optlen = optlen;
        unsafe { *sqe.__bindgen_anon_6.optval.as_mut() = optval as u64 };
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.uring_cmd_flags = flags;
    }
}

opcode! {
    /// Attempt to cancel an already issued request.
    pub struct AsyncCancel = sys::IORING_OP_ASYNC_CANCEL;

    fn new(sqe, user_data: u64) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = user_data;
    }
}

opcode! {
    /// This request must be linked with another request through
    /// [`Flags::IO_LINK`](crate::squeue::Flags::IO_LINK) which is described below.
    /// Unlike [`Timeout`], [`LinkTimeout`] acts on the linked request, not the completion queue.
    pub struct LinkTimeout = sys::IORING_OP_LINK_TIMEOUT;

    fn new(sqe, timespec: *const types::Timespec) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = timespec as _;
        sqe.len = 1;
    }

    fn flags(sqe, flags: types::TimeoutFlags) {
        sqe.__bindgen_anon_3.timeout_flags = flags.bits();
    }
}

opcode! {
    /// Connect a socket, equivalent to `connect(2)`.
    pub struct Connect = sys::IORING_OP_CONNECT;

    fn new(sqe, fd: impl sealed::UseFixed, addr: *const libc::sockaddr, addrlen: libc::socklen_t) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = addr as _;
        sqe.__bindgen_anon_1.off = addrlen as _;
    }
}

// === 5.6 ===

opcode! {
    /// Preallocate or deallocate space to a file, equivalent to `fallocate(2)`.
    pub struct Fallocate = sys::IORING_OP_FALLOCATE;

    fn new(sqe, fd: impl sealed::UseFixed, len: u64) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = len;
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    fn mode(sqe, mode: i32) {
        sqe.len = mode as _;
    }
}

opcode! {
    /// Open a file, equivalent to `openat(2)`.
    pub struct OpenAt = sys::IORING_OP_OPENAT;

    fn new(sqe, dirfd: impl sealed::UseFd, pathname: *const libc::c_char) {
        sqe.fd = sealed::UseFd::into(dirfd);
        sqe.__bindgen_anon_2.addr = pathname as _;
    }

    fn file_index(sqe, file_index: Option<types::DestinationSlot>) {
        sqe.__bindgen_anon_5.file_index = file_index.map_or(0, |d| d.kernel_index_arg());
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.open_flags = flags as _;
    }

    fn mode(sqe, mode: libc::mode_t) {
        sqe.len = mode;
    }
}

opcode! {
    /// Close a file descriptor, equivalent to `close(2)`.
    ///
    /// Use a types::Fixed(fd) argument to close an io_uring direct descriptor.
    pub struct Close = sys::IORING_OP_CLOSE;

    fn new(sqe, fd: impl sealed::UseFixed) {
        match sealed::UseFixed::into(fd) {
            sealed::Target::Fd(fd) => sqe.fd = fd,
            sealed::Target::Fixed(idx) => {
                sqe.fd = 0;
                sqe.__bindgen_anon_5.file_index = idx + 1;
            }
        }
    }
}

opcode! {
    /// This command is an alternative to using
    /// [`Submitter::register_files_update`](crate::Submitter::register_files_update) which then
    /// works in an async fashion, like the rest of the io_uring commands.
    pub struct FilesUpdate = sys::IORING_OP_FILES_UPDATE;

    fn new(sqe, fds: *const RawFd, len: u32) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = fds as _;
        sqe.len = len;
    }

    fn offset(sqe, offset: i32) {
        sqe.__bindgen_anon_1.off = offset as _;
    }
}

opcode! {
    /// Get file status, equivalent to `statx(2)`.
    pub struct Statx = sys::IORING_OP_STATX;

    fn new(
        sqe,
        dirfd: impl sealed::UseFd,
        pathname: *const libc::c_char,
        statxbuf: *mut types::statx,
    ) {
        sqe.fd = sealed::UseFd::into(dirfd);
        sqe.__bindgen_anon_2.addr = pathname as _;
        sqe.__bindgen_anon_1.off = statxbuf as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.statx_flags = flags as _;
    }

    fn mask(sqe, mask: u32) {
        sqe.len = mask;
    }
}

opcode! {
    /// Issue the equivalent of a `pread(2)` or `pwrite(2)` system call
    ///
    /// * `fd` is the file descriptor to be operated on,
    /// * `addr` contains the buffer in question,
    /// * `len` contains the length of the IO operation,
    ///
    /// These are non-vectored versions of the `IORING_OP_READV` and `IORING_OP_WRITEV` opcodes.
    /// See also `read(2)` and `write(2)` for the general description of the related system call.
    ///
    /// Available since 5.6.
    pub struct Read = sys::IORING_OP_READ;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *mut u8, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
    }

    /// `offset` contains the read or write offset.
    ///
    /// If `fd` does not refer to a seekable file, `offset` must be set to zero.
    /// If `offset` is set to `-1`, the offset will use (and advance) the file position,
    /// like the `read(2)` and `write(2)` system calls.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    fn buf_group(sqe, buf_group: u16) {
        sqe.__bindgen_anon_4.buf_group = buf_group;
    }
}

opcode! {
    /// Issue the equivalent of a `pread(2)` or `pwrite(2)` system call
    ///
    /// * `fd` is the file descriptor to be operated on,
    /// * `addr` contains the buffer in question,
    /// * `len` contains the length of the IO operation,
    ///
    /// These are non-vectored versions of the `IORING_OP_READV` and `IORING_OP_WRITEV` opcodes.
    /// See also `read(2)` and `write(2)` for the general description of the related system call.
    ///
    /// Available since 5.6.
    pub struct Write = sys::IORING_OP_WRITE;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *const u8, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
    }

    /// `offset` contains the read or write offset.
    ///
    /// If `fd` does not refer to a seekable file, `offset` must be set to zero.
    /// If `offset` is set to `-1`, the offset will use (and advance) the file position,
    /// like the `read(2)` and `write(2)` system calls.
    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    /// `write_stream` identifies a write stream, added in Linux 6.16
    /// (used by NVMe FDP, for example). `0` means no stream.
    /// Older kernels ignore this field.
    fn write_stream(sqe, write_stream: u8) {
        sqe.__bindgen_anon_5.__bindgen_anon_2.write_stream = write_stream;
    }
}

opcode! {
    /// Predeclare an access pattern for file data, equivalent to `posix_fadvise(2)`.
    pub struct Fadvise = sys::IORING_OP_FADVISE;

    fn new(sqe, fd: impl sealed::UseFixed, len: libc::off_t, advice: i32) {
        assign_fd!(sqe, fd);
        sqe.len = len as _;
        sqe.__bindgen_anon_3.fadvise_advice = advice as _;
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }
}

opcode! {
    /// Give advice about use of memory, equivalent to `madvise(2)`.
    pub struct Madvise = sys::IORING_OP_MADVISE;

    fn new(sqe, addr: *const libc::c_void, len: libc::off_t, advice: i32) {
        sqe.fd = -1;
        sqe.__bindgen_anon_2.addr = addr as _;
        sqe.len = len as _;
        sqe.__bindgen_anon_3.fadvise_advice = advice as _;
    }
}

opcode! {
    /// Send a message on a socket, equivalent to `send(2)`.
    pub struct Send = sys::IORING_OP_SEND;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *const u8, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }

    /// Set the destination address, for sending from an unconnected socket.
    ///
    /// When set, `dest_addr_len` must be set as well.
    /// See also `man 3 io_uring_prep_send_set_addr`.
    fn dest_addr(sqe, dest_addr: *const libc::sockaddr) {
        sqe.__bindgen_anon_1.addr2 = dest_addr as _;
    }

    fn dest_addr_len(sqe, dest_addr_len: libc::socklen_t) {
        sqe.__bindgen_anon_5.__bindgen_anon_1.addr_len = dest_addr_len as _;
    }
}

opcode! {
    /// Receive a message from a socket, equivalent to `recv(2)`.
    pub struct Recv = sys::IORING_OP_RECV;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *mut u8, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }

    fn buf_group(sqe, buf_group: u16) {
        sqe.__bindgen_anon_4.buf_group = buf_group;
    }

    /// Receive into a buffer selected from the provided buffer group `buf_group`.
    ///
    /// Sets `buf_group` and the [`BUFFER_SELECT`](crate::squeue::Flags::BUFFER_SELECT)
    /// flag, the length limits the size of the selected buffer.
    fn buffer_select(sqe, buf_group: u16) {
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
    }
}

opcode! {
    /// Receive multiple messages from a socket, equivalent to `recv(2)`.
    ///
    /// Parameter:
    ///     buf_group: The id of the provided buffer pool to use for each received message.
    ///
    /// MSG_WAITALL should not be set in flags.
    ///
    /// The multishot version allows the application to issue a single receive request, which
    /// repeatedly posts a CQE when data is available. Each CQE will take a buffer out of a
    /// provided buffer pool for receiving. The application should check the flags of each CQE,
    /// regardless of its result. If a posted CQE does not have the IORING_CQE_F_MORE flag set then
    /// the multishot receive will be done and the application should issue a new request.
    ///
    /// Multishot variants are available since kernel 6.0.
    pub struct RecvMulti = sys::IORING_OP_RECV;

    fn new(sqe, fd: impl sealed::UseFixed, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
        sqe.ioprio = sys::IORING_RECV_MULTISHOT as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }

    fn len(sqe, len: u32) {
        sqe.len = len;
    }
}

opcode! {
    /// Open a file, equivalent to `openat2(2)`.
    pub struct OpenAt2 = sys::IORING_OP_OPENAT2;

    fn new(
        sqe,
        dirfd: impl sealed::UseFd,
        pathname: *const libc::c_char,
        how: *const types::OpenHow,
    ) {
        sqe.fd = sealed::UseFd::into(dirfd);
        sqe.__bindgen_anon_2.addr = pathname as _;
        sqe.len = mem::size_of::<sys::open_how>() as _;
        sqe.__bindgen_anon_1.off = how as _;
    }

    fn file_index(sqe, file_index: Option<types::DestinationSlot>) {
        sqe.__bindgen_anon_5.file_index = file_index.map_or(0, |d| d.kernel_index_arg());
    }
}

opcode! {
    /// Modify an epoll file descriptor, equivalent to `epoll_ctl(2)`.
    pub struct EpollCtl = sys::IORING_OP_EPOLL_CTL;

    fn new(
        sqe,
        epfd: impl sealed::UseFixed,
        fd: impl sealed::UseFd,
        op: i32,
        ev: *const types::epoll_event,
    ) {
        assign_fd!(sqe, epfd);
        sqe.__bindgen_anon_2.addr = ev as _;
        sqe.len = op as _;
        sqe.__bindgen_anon_1.off = sealed::UseFd::into(fd) as _;
    }
}

// === 5.7 ===

opcode! {
    /// Splice data to/from a pipe, equivalent to `splice(2)`.
    ///
    /// if `fd_in` refers to a pipe, `off_in` must be `-1`;
    /// The description of `off_in` also applied to `off_out`.
    pub struct Splice = sys::IORING_OP_SPLICE;

    fn new(
        sqe,
        fd_in: impl sealed::UseFixed,
        off_in: i64,
        fd_out: impl sealed::UseFixed,
        off_out: i64,
        len: u32,
    ) {
        assign_fd!(sqe, fd_out);
        sqe.len = len;
        sqe.__bindgen_anon_1.off = off_out as _;
        sqe.__bindgen_anon_5.splice_fd_in = match sealed::UseFixed::into(fd_in) {
            sealed::Target::Fd(fd) => fd,
            sealed::Target::Fixed(idx) => {
                sqe.__bindgen_anon_3.splice_flags = sys::SPLICE_F_FD_IN_FIXED;
                idx as _
            }
        };
        sqe.__bindgen_anon_2.splice_off_in = off_in as _;
    }

    /// see man `splice(2)` for description of flags, ORed with already set flags.
    fn flags(sqe, flags: u32) {
        unsafe { sqe.__bindgen_anon_3.splice_flags |= flags };
    }
}

opcode! {
    /// Register `nbufs` buffers that each have the length `len` with ids starting from `bid` in the
    /// group `bgid` that can be used for any request. See
    /// [`BUFFER_SELECT`](crate::squeue::Flags::BUFFER_SELECT) for more info.
    pub struct ProvideBuffers = sys::IORING_OP_PROVIDE_BUFFERS;

    fn new(sqe, addr: *mut u8, len: i32, nbufs: u16, bgid: u16, bid: u16) {
        sqe.fd = nbufs as _;
        sqe.__bindgen_anon_2.addr = addr as _;
        sqe.len = len as _;
        sqe.__bindgen_anon_1.off = bid as _;
        sqe.__bindgen_anon_4.buf_group = bgid;
    }
}

opcode! {
    /// Remove some number of buffers from a buffer group. See
    /// [`BUFFER_SELECT`](crate::squeue::Flags::BUFFER_SELECT) for more info.
    pub struct RemoveBuffers = sys::IORING_OP_REMOVE_BUFFERS;

    fn new(sqe, nbufs: u16, bgid: u16) {
        sqe.fd = nbufs as _;
        sqe.__bindgen_anon_4.buf_group = bgid;
    }
}

// === 5.8 ===

opcode! {
    /// Duplicate pipe content, equivalent to `tee(2)`.
    pub struct Tee = sys::IORING_OP_TEE;

    fn new(sqe, fd_in: impl sealed::UseFixed, fd_out: impl sealed::UseFixed, len: u32) {
        assign_fd!(sqe, fd_out);
        sqe.len = len;
        sqe.__bindgen_anon_5.splice_fd_in = match sealed::UseFixed::into(fd_in) {
            sealed::Target::Fd(fd) => fd,
            sealed::Target::Fixed(idx) => {
                sqe.__bindgen_anon_3.splice_flags = sys::SPLICE_F_FD_IN_FIXED;
                idx as _
            }
        };
    }

    /// Flags, ORed with already set flags.
    fn flags(sqe, flags: u32) {
        unsafe { sqe.__bindgen_anon_3.splice_flags |= flags };
    }
}

// === 5.11 ===

opcode! {
    /// Shut down all or part of a full duplex connection on a socket, equivalent to `shutdown(2)`.
    /// Available since kernel 5.11.
    pub struct Shutdown = sys::IORING_OP_SHUTDOWN;

    fn new(sqe, fd: impl sealed::UseFixed, how: i32) {
        assign_fd!(sqe, fd);
        sqe.len = how as _;
    }
}

opcode! {
    /// Change the name or location of a file, equivalent to `renameat2(2)`.
    /// Available since kernel 5.11.
    pub struct RenameAt = sys::IORING_OP_RENAMEAT;

    fn new(
        sqe,
        olddirfd: impl sealed::UseFd,
        oldpath: *const libc::c_char,
        newdirfd: impl sealed::UseFd,
        newpath: *const libc::c_char,
    ) {
        sqe.fd = sealed::UseFd::into(olddirfd);
        sqe.__bindgen_anon_2.addr = oldpath as _;
        sqe.len = sealed::UseFd::into(newdirfd) as _;
        sqe.__bindgen_anon_1.off = newpath as _;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.rename_flags = flags;
    }
}

opcode! {
    /// Delete a name and possible the file it refers to, equivalent to `unlinkat(2)`.
    /// Available since kernel 5.11.
    pub struct UnlinkAt = sys::IORING_OP_UNLINKAT;

    fn new(sqe, dirfd: impl sealed::UseFd, pathname: *const libc::c_char) {
        sqe.fd = sealed::UseFd::into(dirfd);
        sqe.__bindgen_anon_2.addr = pathname as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.unlink_flags = flags as _;
    }
}

// === 5.15 ===

opcode! {
    /// Make a directory, equivalent to `mkdirat(2)`.
    pub struct MkDirAt = sys::IORING_OP_MKDIRAT;

    fn new(sqe, dirfd: impl sealed::UseFd, pathname: *const libc::c_char) {
        sqe.fd = sealed::UseFd::into(dirfd);
        sqe.__bindgen_anon_2.addr = pathname as _;
    }

    fn mode(sqe, mode: libc::mode_t) {
        sqe.len = mode;
    }
}

opcode! {
    /// Create a symlink, equivalent to `symlinkat(2)`.
    pub struct SymlinkAt = sys::IORING_OP_SYMLINKAT;

    fn new(
        sqe,
        newdirfd: impl sealed::UseFd,
        target: *const libc::c_char,
        linkpath: *const libc::c_char,
    ) {
        sqe.fd = sealed::UseFd::into(newdirfd);
        sqe.__bindgen_anon_2.addr = target as _;
        sqe.__bindgen_anon_1.addr2 = linkpath as _;
    }
}

opcode! {
    /// Create a hard link, equivalent to `linkat(2)`.
    pub struct LinkAt = sys::IORING_OP_LINKAT;

    fn new(
        sqe,
        olddirfd: impl sealed::UseFd,
        oldpath: *const libc::c_char,
        newdirfd: impl sealed::UseFd,
        newpath: *const libc::c_char,
    ) {
        sqe.fd = sealed::UseFd::into(olddirfd);
        sqe.__bindgen_anon_2.addr = oldpath as _;
        sqe.len = sealed::UseFd::into(newdirfd) as _;
        sqe.__bindgen_anon_1.addr2 = newpath as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.hardlink_flags = flags as _;
    }
}

// === 5.17 ===

opcode! {
    /// Get extended attribute, equivalent to `getxattr(2)`.
    pub struct GetXattr = sys::IORING_OP_GETXATTR;

    fn new(
        sqe,
        name: *const libc::c_char,
        value: *mut libc::c_void,
        path: *const libc::c_char,
        len: u32,
    ) {
        sqe.__bindgen_anon_2.addr = name as _;
        sqe.len = len;
        sqe.__bindgen_anon_1.off = value as _;
        unsafe { sqe.__bindgen_anon_6.__bindgen_anon_1.as_mut().addr3 = path as _ };
    }
}

opcode! {
    /// Set extended attribute, equivalent to `setxattr(2)`.
    pub struct SetXattr = sys::IORING_OP_SETXATTR;

    fn new(
        sqe,
        name: *const libc::c_char,
        value: *const libc::c_void,
        path: *const libc::c_char,
        len: u32,
    ) {
        sqe.__bindgen_anon_2.addr = name as _;
        sqe.len = len;
        sqe.__bindgen_anon_1.off = value as _;
        unsafe { sqe.__bindgen_anon_6.__bindgen_anon_1.as_mut().addr3 = path as _ };
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.xattr_flags = flags as _;
    }
}

opcode! {
    /// Get extended attribute from a file descriptor, equivalent to `fgetxattr(2)`.
    pub struct FGetXattr = sys::IORING_OP_FGETXATTR;

    fn new(
        sqe,
        fd: impl sealed::UseFixed,
        name: *const libc::c_char,
        value: *mut libc::c_void,
        len: u32,
    ) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = name as _;
        sqe.len = len;
        sqe.__bindgen_anon_1.off = value as _;
    }
}

opcode! {
    /// Set extended attribute on a file descriptor, equivalent to `fsetxattr(2)`.
    pub struct FSetXattr = sys::IORING_OP_FSETXATTR;

    fn new(
        sqe,
        fd: impl sealed::UseFixed,
        name: *const libc::c_char,
        value: *const libc::c_void,
        len: u32,
    ) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = name as _;
        sqe.len = len;
        sqe.__bindgen_anon_1.off = value as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.xattr_flags = flags as _;
    }
}

// === 5.18 ===

opcode! {
    /// Send a message (with data) to a target ring.
    pub struct MsgRingData = sys::IORING_OP_MSG_RING;

    fn new(
        sqe,
        ring_fd: impl sealed::UseFd,
        result: i32,
        user_data: u64,
        user_flags: Option<u32>,
    ) {
        sqe.__bindgen_anon_2.addr = sys::IORING_MSG_DATA.into();
        sqe.fd = sealed::UseFd::into(ring_fd);
        sqe.len = result as u32;
        sqe.__bindgen_anon_1.off = user_data;
        if let Some(flags) = user_flags {
            sqe.__bindgen_anon_5.file_index = flags;
            sqe.__bindgen_anon_3.msg_ring_flags = sys::IORING_MSG_RING_FLAGS_PASS;
        }
    }

    /// Flags, ORed with already set flags.
    fn opcode_flags(sqe, opcode_flags: u32) {
        unsafe { sqe.__bindgen_anon_3.msg_ring_flags |= opcode_flags };
    }
}

// === 5.19 ===

opcode! {
    /// Attempt to cancel an already issued request, receiving a cancellation
    /// builder, which allows for the new cancel criterias introduced since
    /// 5.19.
    pub struct AsyncCancel2 = sys::IORING_OP_ASYNC_CANCEL;

    fn new(sqe, builder: types::CancelBuilder) {
        sqe.fd = builder.to_fd();
        sqe.__bindgen_anon_2.addr = builder.user_data.unwrap_or(0);
        sqe.__bindgen_anon_3.cancel_flags = builder.flags.bits();
    }
}

opcode! {
    /// A file/device-specific 16-byte command, akin (but not equivalent) to `ioctl(2)`.
    pub struct UringCmd16 = sys::IORING_OP_URING_CMD;

    fn new(sqe, fd: impl sealed::UseFixed, cmd_op: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_1.__bindgen_anon_1.cmd_op = cmd_op;
    }

    /// The `buf_index` is an index into an array of fixed buffers,
    /// and is only valid if fixed buffers were registered.
    fn buf_index(sqe, buf_index: Option<u16>) {
        if let Some(buf_index) = buf_index {
            sqe.__bindgen_anon_4.buf_index = buf_index;
            unsafe { sqe.__bindgen_anon_3.uring_cmd_flags |= sys::IORING_URING_CMD_FIXED };
        }
    }

    /// Arbitrary command data.
    fn cmd(sqe, cmd: [u8; 16]) {
        unsafe { *sqe.__bindgen_anon_6.cmd.as_mut().as_mut_ptr().cast::<[u8; 16]>() = cmd };
    }

    /// The `addr` is typically a pointer to buffer or iovecs,
    /// but some file/device command also need to pass an addr.
    /// For example, the [ublk](https://docs.kernel.org/block/ublk.html#usage-requirements)
    /// needs to set `addr` to some special value.
    fn addr(sqe, addr: Option<u64>) {
        if let Some(addr) = addr {
            sqe.__bindgen_anon_2.addr = addr;
        }
    }
}

/// A file/device-specific 80-byte command, akin (but not equivalent) to `ioctl(2)`.
///
/// Requires an [`Entry128`] submission queue.
pub struct UringCmd80<'a> {
    sqe: &'a mut sys::io_uring_sqe,
    cmd2: &'a mut [u8; 64],
}

impl<'a> UringCmd80<'a> {
    /// The opcode of the operation. This can be passed to
    /// [`Probe::is_supported`](crate::Probe::is_supported) to check if this operation is
    /// supported with the current kernel.
    pub const CODE: u8 = sys::IORING_OP_URING_CMD as _;

    #[inline]
    pub fn new(entry: &'a mut Entry128, fd: impl sealed::UseFixed, cmd_op: u32) -> Self {
        let Entry128(entry, cmd2) = entry;
        let sqe = &mut entry.0;
        sqe.opcode = Self::CODE;
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_1.__bindgen_anon_1.cmd_op = cmd_op;
        Self { sqe, cmd2 }
    }

    /// The `buf_index` is an index into an array of fixed buffers,
    /// and is only valid if fixed buffers were registered.
    #[inline]
    pub fn buf_index(self, buf_index: Option<u16>) -> Self {
        if let Some(buf_index) = buf_index {
            self.sqe.__bindgen_anon_4.buf_index = buf_index;
            unsafe { self.sqe.__bindgen_anon_3.uring_cmd_flags |= sys::IORING_URING_CMD_FIXED };
        }
        self
    }

    /// Arbitrary command data.
    #[inline]
    pub fn cmd(self, cmd: [u8; 80]) -> Self {
        let cmd1: [u8; 16] = cmd[..16].try_into().unwrap();
        unsafe {
            *self
                .sqe
                .__bindgen_anon_6
                .cmd
                .as_mut()
                .as_mut_ptr()
                .cast::<[u8; 16]>() = cmd1
        };
        self.cmd2.copy_from_slice(&cmd[16..]);
        self
    }

    /// The `addr` is typically a pointer to buffer or iovecs,
    /// but some file/device command also need to pass an addr.
    /// For example, the [ublk](https://docs.kernel.org/block/ublk.html#usage-requirements)
    /// needs to set `addr` to some special value.
    #[inline]
    pub fn addr(self, addr: Option<u64>) -> Self {
        if let Some(addr) = addr {
            self.sqe.__bindgen_anon_2.addr = addr;
        }
        self
    }

    opcode_common!();
}

opcode! {
    /// Create an endpoint for communication, equivalent to `socket(2)`.
    ///
    /// If the `file_index` argument is set, the resulting socket is
    /// directly mapped to the given fixed-file slot instead of being
    /// returned as a normal file descriptor. The application must first
    /// have registered a file table, and the target slot should fit into
    /// it.
    ///
    /// Available since 5.19.
    pub struct Socket = sys::IORING_OP_SOCKET;

    fn new(sqe, domain: i32, socket_type: i32, protocol: i32) {
        sqe.fd = domain as _;
        sqe.__bindgen_anon_1.off = socket_type as _;
        sqe.len = protocol as _;
    }

    fn file_index(sqe, file_index: Option<types::DestinationSlot>) {
        sqe.__bindgen_anon_5.file_index = file_index.map_or(0, |d| d.kernel_index_arg());
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = flags as _;
    }
}

opcode! {
    /// Accept multiple new connections on a socket.
    ///
    /// Set the `allocate_file_index` property if fixed file table entries should be used.
    ///
    /// Available since 5.19.
    pub struct AcceptMulti = sys::IORING_OP_ACCEPT;

    fn new(sqe, fd: impl sealed::UseFixed) {
        assign_fd!(sqe, fd);
        // No out SockAddr is passed for the multishot accept case.
        // The user should perform a syscall to get any resulting connection's remote address.
        sqe.ioprio = sys::IORING_ACCEPT_MULTISHOT as u16;
    }

    fn allocate_file_index(sqe, allocate_file_index: bool) {
        sqe.__bindgen_anon_5.file_index = if allocate_file_index {
            sys::IORING_FILE_INDEX_ALLOC as u32
        } else {
            0
        };
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.accept_flags = flags as _;
    }
}

// === 6.0 ===

opcode! {
    /// Send a message (with fixed FD) to a target ring.
    pub struct MsgRingSendFd = sys::IORING_OP_MSG_RING;

    fn new(
        sqe,
        ring_fd: impl sealed::UseFd,
        fixed_slot_src: types::Fixed,
        dest_slot_index: types::DestinationSlot,
        user_data: u64,
    ) {
        sqe.__bindgen_anon_2.addr = sys::IORING_MSG_SEND_FD.into();
        sqe.fd = sealed::UseFd::into(ring_fd);
        sqe.__bindgen_anon_1.off = user_data;
        unsafe { sqe.__bindgen_anon_6.__bindgen_anon_1.as_mut().addr3 = fixed_slot_src.0 as u64 };
        sqe.__bindgen_anon_5.file_index = dest_slot_index.kernel_index_arg();
    }

    fn opcode_flags(sqe, opcode_flags: u32) {
        sqe.__bindgen_anon_3.msg_ring_flags = opcode_flags;
    }
}

opcode! {
    /// Send a zerocopy message on a socket, equivalent to `send(2)`.
    ///
    /// When `dest_addr` is non-zero it points to the address of the target with `dest_addr_len`
    /// specifying its size, turning the request into a `sendto(2)`
    ///
    /// A fixed (pre-mapped) buffer can optionally be used from pre-mapped buffers that have been
    /// previously registered with [`Submitter::register_buffers`](crate::Submitter::register_buffers).
    ///
    /// This operation might result in two completion queue entries.
    /// See the `IORING_OP_SEND_ZC` section at [io_uring_enter][] for the exact semantics.
    /// Notifications posted by this operation can be checked with [notif](crate::cqueue::notif).
    ///
    /// [io_uring_enter]: https://man7.org/linux/man-pages/man2/io_uring_enter.2.html
    pub struct SendZc = sys::IORING_OP_SEND_ZC;

    fn new(sqe, fd: impl sealed::UseFixed, buf: *const u8, len: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = buf as _;
        sqe.len = len;
    }

    /// The `buf_index` is an index into an array of fixed buffers, and is only valid if fixed
    /// buffers were registered.
    ///
    /// The buf and len arguments must fall within a region specified by buf_index in the
    /// previously registered buffer. The buffer need not be aligned with the start of the
    /// registered buffer.
    fn buf_index(sqe, buf_index: Option<u16>) {
        if let Some(buf_index) = buf_index {
            sqe.__bindgen_anon_4.buf_index = buf_index;
            sqe.ioprio |= sys::IORING_RECVSEND_FIXED_BUF as u16;
        }
    }

    /// Set the destination address, for sending from an unconnected socket.
    ///
    /// When set, `dest_addr_len` must be set as well.
    fn dest_addr(sqe, dest_addr: *const libc::sockaddr) {
        sqe.__bindgen_anon_1.addr2 = dest_addr as _;
    }

    fn dest_addr_len(sqe, dest_addr_len: libc::socklen_t) {
        sqe.__bindgen_anon_5.__bindgen_anon_1.addr_len = dest_addr_len as _;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }

    /// Zerocopy flags, ORed with already set bits.
    fn zc_flags(sqe, zc_flags: u16) {
        sqe.ioprio |= zc_flags;
    }
}

// === 6.1 ===

opcode! {
    /// Send a zerocopy message on a socket, equivalent to `sendmsg(2)`.
    ///
    /// fd must be set to the socket file descriptor, msg must contain a pointer to the msghdr
    /// structure, and flags holds the flags associated with the system call.
    /// The message header must stay valid until the operation completes.
    ///
    /// This operation might result in two completion queue entries.
    /// See the `IORING_OP_SENDMSG_ZC` section at [io_uring_enter][] for the exact semantics.
    /// Notifications posted by this operation can be checked with [notif](crate::cqueue::notif).
    ///
    /// [io_uring_enter]: https://man7.org/linux/man-pages/man2/io_uring_enter.2.html
    pub struct SendMsgZc = sys::IORING_OP_SENDMSG_ZC;

    fn new(sqe, fd: impl sealed::UseFixed, msg: *const libc::msghdr) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = msg as _;
        sqe.len = 1;
    }

    /// I/O priority (zerocopy flags), ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.msg_flags = flags;
    }
}

// === 6.7 ===

opcode! {
    /// Issue the equivalent of `pread(2)` with multi-shot semantics.
    pub struct ReadMulti = sys::IORING_OP_READ_MULTISHOT;

    fn new(sqe, fd: impl sealed::UseFixed, len: u32, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.len = len;
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }
}

opcode! {
    /// Wait on a futex, like but not equivalant to `futex(2)`'s `FUTEX_WAIT_BITSET`.
    ///
    /// Wait on a futex at address `futex` and which still has the value `val` and with `futex2(2)`
    /// flags of `futex_flags`. `musk` can be set to a specific bitset mask, which will be matched
    /// by the waking side to decide who to wake up. To always get woken, an application may use
    /// `FUTEX_BITSET_MATCH_ANY` (truncated to futex bits). `futex_flags` follows the `futex2(2)`
    /// flags, not the `futex(2)` v1 interface flags. `flags` are currently unused and hence `0`
    /// must be passed.
    pub struct FutexWait = sys::IORING_OP_FUTEX_WAIT;

    fn new(sqe, futex: *const u32, val: u64, mask: u64, futex_flags: u32) {
        sqe.fd = futex_flags as _;
        sqe.__bindgen_anon_2.addr = futex as usize as _;
        sqe.__bindgen_anon_1.off = val;
        unsafe { sqe.__bindgen_anon_6.__bindgen_anon_1.as_mut().addr3 = mask };
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.futex_flags = flags;
    }
}

opcode! {
    /// Wake up waiters on a futex, like but not equivalant to `futex(2)`'s `FUTEX_WAKE_BITSET`.
    ///
    /// Wake any waiters on the futex indicated by `futex` and at most `val` futexes. `futex_flags`
    /// indicates the `futex2(2)` modifier flags. If a given bitset for who to wake is desired,
    /// then that must be set in `mask`. Use `FUTEX_BITSET_MATCH_ANY` (truncated to futex bits) to
    /// match any waiter on the given futex. `flags` are currently unused and hence `0` must be
    /// passed.
    pub struct FutexWake = sys::IORING_OP_FUTEX_WAKE;

    fn new(sqe, futex: *const u32, val: u64, mask: u64, futex_flags: u32) {
        sqe.fd = futex_flags as _;
        sqe.__bindgen_anon_2.addr = futex as usize as _;
        sqe.__bindgen_anon_1.off = val;
        unsafe { sqe.__bindgen_anon_6.__bindgen_anon_1.as_mut().addr3 = mask };
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.futex_flags = flags;
    }
}

opcode! {
    /// Wait on multiple futexes.
    ///
    /// Wait on multiple futexes at the same time. Futexes are given by `futexv` and `nr_futex` is
    /// the number of futexes in that array. Unlike `FutexWait`, the desired bitset mask and values
    /// are passed in `futexv`. `flags` are currently unused and hence `0` must be passed.
    pub struct FutexWaitV = sys::IORING_OP_FUTEX_WAITV;

    fn new(sqe, futexv: *const types::FutexWaitV, nr_futex: u32) {
        sqe.__bindgen_anon_2.addr = futexv as usize as _;
        sqe.len = nr_futex;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.futex_flags = flags;
    }
}

opcode! {
    /// Issue the equivalent of a `waitid(2)` system call.
    ///
    /// Available since kernel 6.7.
    pub struct WaitId = sys::IORING_OP_WAITID;

    fn new(sqe, idtype: libc::idtype_t, id: libc::id_t, options: libc::c_int) {
        sqe.fd = id as _;
        sqe.len = idtype as _;
        sqe.__bindgen_anon_5.file_index = options as _;
    }

    fn infop(sqe, infop: *const libc::siginfo_t) {
        sqe.__bindgen_anon_1.addr2 = infop as _;
    }

    fn flags(sqe, flags: libc::c_uint) {
        sqe.__bindgen_anon_3.waitid_flags = flags;
    }
}

// === 6.8 ===

opcode! {
    /// Install a fixed file descriptor
    ///
    /// Turns a direct descriptor into a regular file descriptor that can be later used by regular
    /// system calls that take a normal raw file descriptor
    pub struct FixedFdInstall = sys::IORING_OP_FIXED_FD_INSTALL;

    fn new(sqe, fd: types::Fixed, file_flags: u32) {
        sqe.fd = fd.0 as _;
        sqe.flags |= Flags::FIXED_FILE.bits();
        sqe.__bindgen_anon_3.install_fd_flags = file_flags;
    }
}

// === 6.9 ===

opcode! {
    /// Perform file truncation, equivalent to `ftruncate(2)`.
    pub struct Ftruncate = sys::IORING_OP_FTRUNCATE;

    fn new(sqe, fd: impl sealed::UseFixed, len: u64) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_1.off = len;
    }
}

// === 6.10 ===

opcode! {
    /// Send a bundle of messages on a socket in a single request.
    pub struct SendBundle = sys::IORING_OP_SEND;

    fn new(sqe, fd: impl sealed::UseFixed, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.ioprio = sys::IORING_RECVSEND_BUNDLE as u16;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
        sqe.__bindgen_anon_4.buf_group = buf_group;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }

    fn len(sqe, len: u32) {
        sqe.len = len;
    }
}

opcode! {
    /// Receive a bundle of buffers from a socket.
    ///
    /// Parameter
    ///     buf_group: The id of the provided buffer pool to use for the bundle.
    ///
    /// Note that as of kernel 6.10 first recv always gets a single buffer, while second
    /// obtains the bundle of remaining buffers. This behavior may change in the future.
    ///
    /// Bundle variant is available since kernel 6.10
    pub struct RecvBundle = sys::IORING_OP_RECV;

    fn new(sqe, fd: impl sealed::UseFixed, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
        sqe.ioprio = sys::IORING_RECVSEND_BUNDLE as u16;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }
}

opcode! {
    /// Receive multiple messages from a socket as a bundle.
    ///
    /// Parameter:
    ///     buf_group: The id of the provided buffer pool to use for each received message.
    ///
    /// MSG_WAITALL should not be set in flags.
    ///
    /// The multishot version allows the application to issue a single receive request, which
    /// repeatedly posts a CQE when data is available. Each CQE will take a bundle of buffers
    /// out of a provided buffer pool for receiving. The application should check the flags of each CQE,
    /// regardless of its result. If a posted CQE does not have the IORING_CQE_F_MORE flag set then
    /// the multishot receive will be done and the application should issue a new request.
    ///
    /// Note that as of kernel 6.10 first CQE always gets a single buffer, while second
    /// obtains the bundle of remaining buffers. This behavior may change in the future.
    ///
    /// Multishot bundle variant is available since kernel 6.10.
    pub struct RecvMultiBundle = sys::IORING_OP_RECV;

    fn new(sqe, fd: impl sealed::UseFixed, buf_group: u16) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_4.buf_group = buf_group;
        sqe.flags |= Flags::BUFFER_SELECT.bits();
        sqe.ioprio = (sys::IORING_RECV_MULTISHOT | sys::IORING_RECVSEND_BUNDLE) as u16;
    }

    fn flags(sqe, flags: i32) {
        sqe.__bindgen_anon_3.msg_flags = flags as _;
    }
}

// === 6.11 ===

opcode! {
    /// Bind a socket, equivalent to `bind(2)`.
    pub struct Bind = sys::IORING_OP_BIND;

    fn new(sqe, fd: impl sealed::UseFixed, addr: *const libc::sockaddr, addrlen: libc::socklen_t) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = addr as _;
        sqe.__bindgen_anon_1.off = addrlen as _;
    }
}

opcode! {
    /// Listen on a socket, equivalent to `listen(2)`.
    pub struct Listen = sys::IORING_OP_LISTEN;

    fn new(sqe, fd: impl sealed::UseFixed, backlog: i32) {
        assign_fd!(sqe, fd);
        sqe.len = backlog as _;
    }
}

// === 6.15 ===

opcode! {
    /// Issue the zerocopy equivalent of a `recv(2)` system call.
    pub struct RecvZc = sys::IORING_OP_RECV_ZC;

    fn new(sqe, fd: impl sealed::UseFixed, len: u32) {
        assign_fd!(sqe, fd);
        sqe.len = len;
        sqe.ioprio = sys::IORING_RECV_MULTISHOT as u16;
    }

    fn ifq(sqe, ifq: u32) {
        sqe.__bindgen_anon_5.zcrx_ifq_idx = ifq;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }
}

opcode! {
    /// Issue the equivalent of a `epoll_wait(2)` system call.
    pub struct EpollWait = sys::IORING_OP_EPOLL_WAIT;

    fn new(sqe, fd: impl sealed::UseFixed, events: *mut types::epoll_event, max_events: u32) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = events as u64;
        sqe.len = max_events;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.poll32_events = flags;
    }
}

opcode! {
    /// Vectored read into a fixed buffer, equivalent to `preadv2(2)`.
    pub struct ReadvFixed = sys::IORING_OP_READV_FIXED;

    fn new(
        sqe,
        fd: impl sealed::UseFixed,
        iovec: *const libc::iovec,
        len: u32,
        buf_index: u16,
    ) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = iovec as _;
        sqe.len = len;
        sqe.__bindgen_anon_4.buf_index = buf_index;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }
}

opcode! {
    /// Vectored write from a fixed buffer, equivalent to `pwritev2(2)`.
    pub struct WritevFixed = sys::IORING_OP_WRITEV_FIXED;

    fn new(
        sqe,
        fd: impl sealed::UseFixed,
        iovec: *const libc::iovec,
        len: u32,
        buf_index: u16,
    ) {
        assign_fd!(sqe, fd);
        sqe.__bindgen_anon_2.addr = iovec as _;
        sqe.len = len;
        sqe.__bindgen_anon_4.buf_index = buf_index;
    }

    /// I/O priority, ORed with already set bits.
    fn ioprio(sqe, ioprio: u16) {
        sqe.ioprio |= ioprio;
    }

    fn offset(sqe, offset: u64) {
        sqe.__bindgen_anon_1.off = offset;
    }

    fn rw_flags(sqe, rw_flags: i32) {
        sqe.__bindgen_anon_3.rw_flags = rw_flags as _;
    }

    /// Write stream id (Linux 6.16+); `0` means no stream.
    /// Older kernels ignore this field.
    fn write_stream(sqe, write_stream: u8) {
        sqe.__bindgen_anon_5.__bindgen_anon_2.write_stream = write_stream;
    }
}

// === 6.16 ===

opcode! {
    /// Create a pipe, equivalent to `pipe(2)`.
    pub struct Pipe = sys::IORING_OP_PIPE;

    fn new(sqe, fds: *mut RawFd) {
        sqe.fd = 0;
        sqe.__bindgen_anon_2.addr = fds as _;
    }

    fn flags(sqe, flags: u32) {
        sqe.__bindgen_anon_3.pipe_flags = flags;
    }

    fn file_index(sqe, file_index: Option<types::DestinationSlot>) {
        sqe.__bindgen_anon_5.file_index = file_index.map_or(0, |d| d.kernel_index_arg());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Fd, Fixed};

    #[test]
    fn fixed_fd_sets_sqe_flag() {
        let mut entry = Entry::default();
        Recv::new(&mut entry, Fixed(3), std::ptr::null_mut(), 0);
        assert_eq!(entry.0.opcode, Recv::CODE);
        assert_eq!(entry.0.fd, 3);
        assert_eq!(entry.0.flags, Flags::FIXED_FILE.bits());

        let mut entry = Entry::default();
        Recv::new(&mut entry, Fd(5), std::ptr::null_mut(), 0);
        assert_eq!(entry.0.fd, 5);
        assert_eq!(entry.0.flags, 0);
    }

    #[test]
    fn common_setters() {
        let mut entry = Entry::default();
        Nop::new(&mut entry)
            .user_data(42)
            .sqe_flags(Flags::IO_LINK)
            .sqe_flags(Flags::ASYNC)
            .personality(3);
        assert_eq!(entry.0.opcode, Nop::CODE);
        assert_eq!(entry.0.fd, -1);
        assert_eq!(entry.get_user_data(), 42);
        assert_eq!(entry.0.flags, (Flags::IO_LINK | Flags::ASYNC).bits());
        assert_eq!(entry.0.personality, 3);
    }

    #[test]
    fn recv_buffer_select() {
        let mut entry = Entry::default();
        Recv::new(&mut entry, Fixed(3), std::ptr::null_mut(), 4096)
            .buffer_select(7)
            .ioprio(sys::IORING_RECVSEND_POLL_FIRST as u16);
        assert_eq!(entry.0.len, 4096);
        assert_eq!(unsafe { entry.0.__bindgen_anon_2.addr }, 0);
        assert_eq!(unsafe { entry.0.__bindgen_anon_4.buf_group }, 7);
        assert_eq!(
            entry.0.flags,
            (Flags::FIXED_FILE | Flags::BUFFER_SELECT).bits()
        );
        assert_eq!(entry.0.ioprio, sys::IORING_RECVSEND_POLL_FIRST as u16);
    }

    #[test]
    fn sendmsg_sets_msg() {
        let hdr = std::mem::MaybeUninit::<libc::msghdr>::zeroed();
        let msg = hdr.as_ptr();

        let mut entry = Entry::default();
        SendMsg::new(&mut entry, Fd(4), msg);
        assert_eq!(entry.0.opcode, SendMsg::CODE);
        assert_eq!(entry.0.fd, 4);
        assert_eq!(entry.0.len, 1);
        assert_eq!(unsafe { entry.0.__bindgen_anon_2.addr }, msg as u64);

        let mut entry = Entry::default();
        SendMsgZc::new(&mut entry, Fd(4), msg).ioprio(sys::IORING_SEND_ZC_REPORT_USAGE as u16);
        assert_eq!(entry.0.opcode, SendMsgZc::CODE);
        assert_eq!(entry.0.len, 1);
        assert_eq!(unsafe { entry.0.__bindgen_anon_2.addr }, msg as u64);
        assert_eq!(entry.0.ioprio, sys::IORING_SEND_ZC_REPORT_USAGE as u16);
    }

    #[test]
    fn send_zc_ioprio_bits_combine() {
        let mut entry = Entry::default();
        SendZc::new(&mut entry, Fd(1), std::ptr::null(), 0)
            .buf_index(Some(2))
            .zc_flags(sys::IORING_RECVSEND_POLL_FIRST as u16);
        assert_eq!(unsafe { entry.0.__bindgen_anon_4.buf_index }, 2);
        assert_eq!(
            entry.0.ioprio,
            (sys::IORING_RECVSEND_FIXED_BUF | sys::IORING_RECVSEND_POLL_FIRST) as u16
        );
    }

    #[test]
    fn splice_fixed_fd_in_keeps_flag() {
        let mut entry = Entry::default();
        Splice::new(&mut entry, Fixed(1), -1, Fd(2), -1, 10).flags(libc::SPLICE_F_MOVE);
        assert_eq!(unsafe { entry.0.__bindgen_anon_5.splice_fd_in }, 1);
        assert_eq!(
            unsafe { entry.0.__bindgen_anon_3.splice_flags },
            sys::SPLICE_F_FD_IN_FIXED | libc::SPLICE_F_MOVE
        );
    }

    #[test]
    fn read_multi_fixed_fd_keeps_flag() {
        let mut entry = Entry::default();
        ReadMulti::new(&mut entry, Fixed(1), 0, 3);
        assert_eq!(
            entry.0.flags,
            (Flags::FIXED_FILE | Flags::BUFFER_SELECT).bits()
        );
    }

    #[test]
    fn timeout_update_keeps_update_flag() {
        let ts = types::Timespec::new();
        let mut entry = Entry::default();
        TimeoutUpdate::new(&mut entry, 1, &ts).flags(types::TimeoutFlags::ABS);
        assert_eq!(
            unsafe { entry.0.__bindgen_anon_3.timeout_flags },
            types::TimeoutFlags::ABS.bits() | sys::IORING_TIMEOUT_UPDATE
        );
    }

    #[test]
    fn close_fixed() {
        let mut entry = Entry::default();
        Close::new(&mut entry, Fixed(4));
        assert_eq!(entry.0.fd, 0);
        assert_eq!(entry.0.flags, 0);
        assert_eq!(unsafe { entry.0.__bindgen_anon_5.file_index }, 5);
    }

    #[test]
    fn uring_cmd80_splits_cmd() {
        let mut cmd = [0u8; 80];
        cmd.iter_mut().enumerate().for_each(|(i, b)| *b = i as u8);

        let mut entry = Entry128::from(Entry::default());
        UringCmd80::new(&mut entry, Fd(1), 9).cmd(cmd).user_data(5);
        assert_eq!(entry.0.0.opcode, UringCmd80::CODE);
        assert_eq!(entry.get_user_data(), 5);
        let cmd1 = unsafe {
            *entry
                .0
                .0
                .__bindgen_anon_6
                .cmd
                .as_ref()
                .as_ptr()
                .cast::<[u8; 16]>()
        };
        assert_eq!(cmd1[..], cmd[..16]);
        assert_eq!(entry.1[..], cmd[16..]);
    }

    #[test]
    fn entry128_accepts_ops() {
        let mut entry = Entry128::from(Entry::default());
        Nop::new(&mut entry).user_data(9);
        assert_eq!(entry.get_opcode(), Nop::CODE as u32);
        assert_eq!(entry.get_user_data(), 9);
    }
}

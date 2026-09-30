use crate::{squeue::Entry, sys, types::sealed};

macro_rules! assign_fd {
    ( $sqe:expr, $opfd:expr ) => {{
        let sqe: &mut sys::io_uring_sqe = $sqe;
        match $opfd.into() {
            sealed::Target::Fd(fd) => {
                sqe.fd = fd;
                sqe.flags &= !crate::squeue::Flags::FIXED_FILE.bits();
            }
            sealed::Target::Fixed(idx) => {
                sqe.fd = idx as _;
                sqe.flags |= crate::squeue::Flags::FIXED_FILE.bits();
            }
        }
    }};
}

macro_rules! opcode {
    ($( #[$outer:meta] )*
     $name:ident, $opcode:expr) => {
        $( #[$outer] )*
        pub struct $name<'a> {
            sqe: &'a mut sys::io_uring_sqe,
        }

        impl<'a> $name<'a> {
            pub const CODE: u8 = $opcode as u8;

            pub fn new(entry: &'a mut Entry) -> Self {
                entry.0.opcode = Self::CODE;
                Self { sqe: &mut entry.0 }
            }

            pub fn with(entry: &'a mut Entry, fd: impl sealed::UseFixed) -> Self {
                entry.0.opcode = Self::CODE;
                assign_fd!(&mut entry.0, fd);

                Self { sqe: &mut entry.0 }
            }

            #[doc(hidden)]
            pub fn fd(self, fd: impl sealed::UseFixed) -> Self {
                assign_fd!(&mut *self.sqe, fd);
                self
            }

            pub fn ioprio(self, ioprio: u16) -> Self {
                self.sqe.ioprio = ioprio;
                self
            }

            pub fn flags(self, flags: i32) -> Self {
                unsafe {
                    self.sqe.__bindgen_anon_3.msg_flags |= flags as u32;
                }
                self
            }
        }
    };
}

macro_rules! opcode_buf {
    () => {
        pub fn buffer(self, buf: *mut u8, len: u32) -> Self {
            self.sqe.__bindgen_anon_2.addr = buf as _;
            self.sqe.len = len;
            self
        }

        #[doc(hidden)]
        pub fn buf(self, buf: *mut u8) -> Self {
            self.sqe.__bindgen_anon_2.addr = buf as _;
            self
        }

        #[doc(hidden)]
        pub fn len(self, len: u32) -> Self {
            self.sqe.len = len;
            self
        }
    };
}

macro_rules! opcode_send_buf {
    () => {
        pub fn buffer(self, buf: *const u8, len: u32) -> Self {
            self.sqe.__bindgen_anon_2.addr = buf as _;
            self.sqe.len = len;
            self
        }

        #[doc(hidden)]
        pub fn buf(self, buf: *const u8) -> Self {
            self.sqe.__bindgen_anon_2.addr = buf as _;
            self
        }

        #[doc(hidden)]
        pub fn len(self, len: u32) -> Self {
            self.sqe.len = len;
            self
        }
    };
}

opcode! {
    /// Send a message on a socket, equivalent to `send(2)`.
    Send, sys::IORING_OP_SEND
}

impl<'a> Send<'a> {
    opcode_send_buf!();

    /// Set the destination address, for sending from an unconnected socket.
    ///
    /// When set, `dest_addr_len` must be set as well.
    /// See also `man 3 io_uring_prep_send_set_addr`.
    pub fn dest_addr(self, addr: *const libc::sockaddr, len: libc::socklen_t) -> Self {
        self.sqe.__bindgen_anon_1.addr2 = addr as _;
        self.sqe.__bindgen_anon_5.__bindgen_anon_1.addr_len = len as _;
        self
    }
}

opcode! {
    /// Receive a message from a socket, equivalent to `recv(2)`.
    Recv, sys::IORING_OP_RECV
}

impl<'a> Recv<'a> {
    opcode_buf!();

    pub fn buf_group(self, grp: u16) -> Self {
        self.sqe.__bindgen_anon_4.buf_group = grp;
        self
    }
}

// === 6.0 ===

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
    SendZc, sys::IORING_OP_SEND_ZC
}

impl<'a> SendZc<'a> {
    opcode_send_buf!();

    /// The `buf_index` is an index into an array of fixed buffers, and is only valid if fixed
    /// buffers were registered.
    ///
    /// The buf and len arguments must fall within a region specified by buf_index in the
    /// previously registered buffer. The buffer need not be aligned with the start of the
    /// registered buffer.
    pub fn buf_index(self, idx: u16) -> Self {
        self.sqe.__bindgen_anon_4.buf_index = idx;
        self.sqe.ioprio |= sys::IORING_RECVSEND_FIXED_BUF as u16;
        self
    }

    /// Set the destination address, for sending from an unconnected socket.
    ///
    /// When set, `dest_addr_len` must be set as well.
    /// See also `man 3 io_uring_prep_send_set_addr`.
    pub fn dest_addr(self, addr: *const libc::sockaddr, len: libc::socklen_t) -> Self {
        self.sqe.__bindgen_anon_1.addr2 = addr as _;
        self.sqe.__bindgen_anon_5.__bindgen_anon_1.addr_len = len as _;
        self
    }
}

opcode! {
    /// Vectored write, equivalent to `pwritev2(2)`.
    Writev, sys::IORING_OP_WRITEV
}

impl<'a> Writev<'a> {
    pub fn iovec(self, iovec: *const libc::iovec, nr: u32) -> Self {
        self.sqe.__bindgen_anon_2.addr = iovec as _;
        self.sqe.len = nr;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{squeue::Flags, types};

    #[test]
    fn fixed_fd_sets_sqe_flag() {
        let mut entry = Entry::default();
        Recv::with(&mut entry, types::Fixed(3));
        assert_eq!(entry.0.fd, 3);
        assert_eq!(entry.0.flags, Flags::FIXED_FILE.bits());
        assert_eq!(unsafe { entry.0.__bindgen_anon_3.msg_flags }, 0);

        Recv::new(&mut entry).fd(types::Fd(5));
        assert_eq!(entry.0.fd, 5);
        assert_eq!(entry.0.flags, 0);
        assert_eq!(unsafe { entry.0.__bindgen_anon_3.msg_flags }, 0);
    }
}

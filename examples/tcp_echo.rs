use std::collections::VecDeque;
use std::net::TcpListener;
use std::os::unix::io::{AsRawFd, RawFd};
use std::{io, ptr};

use ntex_io_uring::{IoUring, SubmissionQueue, opcode, squeue, types};
use slab::Slab;

#[derive(Clone, Debug)]
enum Token {
    Accept,
    Poll {
        fd: RawFd,
    },
    Read {
        fd: RawFd,
        buf_index: usize,
    },
    Write {
        fd: RawFd,
        buf_index: usize,
        offset: usize,
        len: usize,
    },
}

pub struct AcceptCount {
    entry: squeue::Entry,
    count: usize,
}

impl AcceptCount {
    fn new(fd: RawFd, token: usize, count: usize) -> AcceptCount {
        let mut entry = squeue::Entry::default();
        opcode::Accept::new(&mut entry, types::Fd(fd), ptr::null_mut(), ptr::null_mut())
            .user_data(token as _);

        AcceptCount { entry, count }
    }

    pub fn push_to(&mut self, sq: &mut SubmissionQueue<'_>) {
        while self.count > 0 {
            unsafe {
                match sq.push(|entry| *entry = self.entry.clone()) {
                    Ok(_) => self.count -= 1,
                    Err(_) => break,
                }
            }
        }

        sq.sync();
    }
}

fn main() -> anyhow::Result<()> {
    let mut ring = IoUring::new(256)?;
    let listener = TcpListener::bind(("127.0.0.1", 3456))?;

    let mut backlog = VecDeque::new();
    let mut bufpool = Vec::with_capacity(64);
    let mut buf_alloc = Slab::with_capacity(64);
    let mut token_alloc = Slab::with_capacity(64);

    println!("listen {}", listener.local_addr()?);

    let (submitter, mut sq, mut cq) = ring.split();

    let mut accept = AcceptCount::new(listener.as_raw_fd(), token_alloc.insert(Token::Accept), 3);

    accept.push_to(&mut sq);

    loop {
        match submitter.submit_and_wait(1) {
            Ok(_) => (),
            Err(ref err) if err.raw_os_error() == Some(libc::EBUSY) => (),
            Err(err) => return Err(err.into()),
        }
        cq.sync();

        // clean backlog
        loop {
            if sq.is_full() {
                match submitter.submit() {
                    Ok(_) => (),
                    Err(ref err) if err.raw_os_error() == Some(libc::EBUSY) => break,
                    Err(err) => return Err(err.into()),
                }
            }
            sq.sync();

            match backlog.pop_front() {
                Some(sqe) => unsafe {
                    let _ = sq.push(|entry| *entry = sqe);
                },
                None => break,
            }
        }

        accept.push_to(&mut sq);

        for cqe in &mut cq {
            let ret = cqe.result();
            let token_index = cqe.user_data() as usize;

            if ret < 0 {
                eprintln!(
                    "token {:?} error: {:?}",
                    token_alloc.get(token_index),
                    io::Error::from_raw_os_error(-ret)
                );
                continue;
            }

            let token = &mut token_alloc[token_index];
            match token.clone() {
                Token::Accept => {
                    println!("accept");

                    accept.count += 1;

                    let fd = ret;
                    let poll_token = token_alloc.insert(Token::Poll { fd });

                    let mut poll_e = squeue::Entry::default();
                    opcode::PollAdd::new(&mut poll_e, types::Fd(fd), libc::POLLIN as _)
                        .user_data(poll_token as _);

                    unsafe {
                        if sq.push(|entry| *entry = poll_e.clone()).is_err() {
                            backlog.push_back(poll_e);
                        }
                    }
                }
                Token::Poll { fd } => {
                    let (buf_index, buf) = match bufpool.pop() {
                        Some(buf_index) => (buf_index, &mut buf_alloc[buf_index]),
                        None => {
                            let buf = vec![0u8; 2048].into_boxed_slice();
                            let buf_entry = buf_alloc.vacant_entry();
                            let buf_index = buf_entry.key();
                            (buf_index, buf_entry.insert(buf))
                        }
                    };

                    *token = Token::Read { fd, buf_index };

                    let mut read_e = squeue::Entry::default();
                    opcode::Recv::new(&mut read_e, types::Fd(fd), buf.as_mut_ptr(), buf.len() as _)
                        .user_data(token_index as _);

                    unsafe {
                        if sq.push(|entry| *entry = read_e.clone()).is_err() {
                            backlog.push_back(read_e);
                        }
                    }
                }
                Token::Read { fd, buf_index } => {
                    if ret == 0 {
                        bufpool.push(buf_index);
                        token_alloc.remove(token_index);

                        println!("shutdown");

                        unsafe {
                            libc::close(fd);
                        }
                    } else {
                        let len = ret as usize;
                        let buf = &buf_alloc[buf_index];

                        *token = Token::Write {
                            fd,
                            buf_index,
                            len,
                            offset: 0,
                        };

                        let mut write_e = squeue::Entry::default();
                        opcode::Send::new(&mut write_e, types::Fd(fd), buf.as_ptr(), len as _)
                            .user_data(token_index as _);

                        unsafe {
                            if sq.push(|entry| *entry = write_e.clone()).is_err() {
                                backlog.push_back(write_e);
                            }
                        }
                    }
                }
                Token::Write {
                    fd,
                    buf_index,
                    offset,
                    len,
                } => {
                    let write_len = ret as usize;

                    let mut entry = squeue::Entry::default();
                    if offset + write_len >= len {
                        bufpool.push(buf_index);

                        *token = Token::Poll { fd };

                        opcode::PollAdd::new(&mut entry, types::Fd(fd), libc::POLLIN as _)
                            .user_data(token_index as _);
                    } else {
                        let offset = offset + write_len;
                        let len = len - offset;

                        let buf = &buf_alloc[buf_index][offset..];

                        *token = Token::Write {
                            fd,
                            buf_index,
                            offset,
                            len,
                        };

                        opcode::Write::new(&mut entry, types::Fd(fd), buf.as_ptr(), len as _)
                            .user_data(token_index as _);
                    };

                    unsafe {
                        if sq.push(|e| *e = entry.clone()).is_err() {
                            backlog.push_back(entry);
                        }
                    }
                }
            }
        }
    }
}

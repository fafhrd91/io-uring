use crate::utils;
use crate::Test;
use ntex_io_uring::{cqueue, opcode, squeue, types, IoUring};
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};

pub fn test_file_write_read<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Write::CODE);
        test.probe.is_supported(opcode::Read::CODE);
    );

    println!("test file_write_read");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.as_raw_fd());

    utils::write_read(ring, fd, fd)?;

    Ok(())
}

pub fn test_file_write_stream<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Write::CODE);
        test.probe.is_supported(opcode::Writev::CODE);
        test.probe.is_supported(opcode::Read::CODE);
    );

    println!("test file_write_stream");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.as_raw_fd());

    let text = b"The quick brown fox jumps over the lazy dog.";
    let mut output = vec![0; text.len()];

    // A nonzero write_stream on a buffered file is accepted by any kernel:
    // 6.16+ carries it down to the storage device, older kernels ignore it.
    unsafe {
        let queue = ring.submission();
        queue
            .push(|entry| {
                opcode::Write::new(entry, fd, text.as_ptr(), text.len() as _)
                    .write_stream(1)
                    .user_data(0x01)
                    .sqe_flags(squeue::Flags::IO_LINK);
            })
            .expect("queue is full");
        queue
            .push(|entry| {
                opcode::Read::new(entry, fd, output.as_mut_ptr(), output.len() as _)
                    .user_data(0x02);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(2)?, 2);

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 2);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[1].user_data(), 0x02);
    assert_eq!(cqes[0].result(), text.len() as i32);
    assert_eq!(cqes[1].result(), text.len() as i32);

    assert_eq!(&output[..cqes[1].result() as usize], text);

    // And via Writev, which has its own write_stream path.
    let iov = ::libc::iovec {
        iov_base: text.as_ptr() as *mut _,
        iov_len: text.len(),
    };

    unsafe {
        let queue = ring.submission();
        queue
            .push(|entry| {
                opcode::Writev::new(entry, fd, &iov, 1)
                    .write_stream(1)
                    .user_data(0x03)
                    .sqe_flags(squeue::Flags::IO_LINK);
            })
            .expect("queue is full");
        queue
            .push(|entry| {
                opcode::Read::new(entry, fd, output.as_mut_ptr(), output.len() as _)
                    .user_data(0x04);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(2)?, 2);

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 2);
    assert_eq!(cqes[0].user_data(), 0x03);
    assert_eq!(cqes[1].user_data(), 0x04);
    assert_eq!(cqes[0].result(), text.len() as i32);
    assert_eq!(cqes[1].result(), text.len() as i32);

    assert_eq!(&output[..cqes[1].result() as usize], text);

    Ok(())
}

pub fn test_pipe_read_multishot<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Write::CODE);
        test.probe.is_supported(opcode::ReadMulti::CODE);
    );

    println!("test pipe_read_multishot");

    use crate::tests::register_buf_ring;
    use ::std::collections::BTreeSet;

    let (rx, tx) = ::std::io::pipe()?;

    const BYTES0: &[u8] = "The quick brown fox jumps over the lazy dog.".as_bytes();
    const BYTES1: &[u8] = "我能吞下玻璃而不伤身体。".as_bytes();

    const REQ_TYPE_READ: u64 = 1;
    const REQ_TYPE_WRITE_BYTES0: u64 = 2;
    const REQ_TYPE_WRITE_BYTES1: u64 = 3;

    let buf_ring = register_buf_ring::Builder::new(0xcafe)
        .ring_entries(2)
        .buf_len(BYTES0.len().max(BYTES1.len()))
        .build()?;
    buf_ring.rc.register(ring)?;

    let mut got_writes;
    let mut got_reads;
    let mut got_bufs: BTreeSet<u16>;

    // Prepare multishot read

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::ReadMulti::new(entry, types::Fd(rx.as_raw_fd()), 0, 0xcafe)
                    .user_data(REQ_TYPE_READ);
            })
            .expect("queue is full");
    }

    // Write BYTES0

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Write::new(
                    entry,
                    types::Fd(tx.as_raw_fd()),
                    BYTES0.as_ptr(),
                    BYTES0.len() as _,
                )
                .user_data(REQ_TYPE_WRITE_BYTES0)
                .sqe_flags(squeue::Flags::IO_LINK);
            })
            .expect("queue is full");
    }
    ring.submit_and_wait(2)?;

    // Process one write/read pair. Fills the first buffer in the ring.

    got_writes = 0;
    got_reads = 0;
    got_bufs = BTreeSet::new();
    for cqe in ring.completion().map(Into::<cqueue::Entry>::into) {
        assert!(cqe.result() >= 0);
        let len = cqe.result().cast_unsigned();
        match cqe.user_data() {
            REQ_TYPE_WRITE_BYTES0 => {
                assert_eq!(BYTES0.len(), len as _);
                got_writes += 1;
            }
            REQ_TYPE_READ => {
                let bufs = buf_ring.rc.get_bufs(&buf_ring, len, cqe.flags());
                assert_eq!(1, bufs.len());
                assert_eq!(Some(0), cqueue::buffer_select(cqe.flags()));
                assert_eq!(BYTES0.len(), len as _);
                assert_eq!(BYTES0, bufs[0].as_slice());
                assert!(cqueue::more(cqe.flags()));
                got_reads += 1;
                got_bufs.insert(0);
            }
            _ => {}
        }
    }
    assert_eq!(got_writes, 1);
    assert_eq!(got_reads, 1);
    assert_eq!(got_bufs, BTreeSet::from([0]));

    // Write BYTES1

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Write::new(
                    entry,
                    types::Fd(tx.as_raw_fd()),
                    BYTES1.as_ptr(),
                    BYTES1.len() as _,
                )
                .user_data(REQ_TYPE_WRITE_BYTES1)
                .sqe_flags(squeue::Flags::IO_LINK);
            })
            .expect("queue is full");
    }
    ring.submit_and_wait(2)?;

    // Process one write/read pair. Fills the first buffer in the ring.

    got_writes = 0;
    got_reads = 0;
    got_bufs = BTreeSet::new();
    for cqe in ring.completion().map(Into::<cqueue::Entry>::into) {
        assert!(cqe.result() >= 0);
        let len = cqe.result().cast_unsigned();
        match cqe.user_data() {
            REQ_TYPE_WRITE_BYTES1 => {
                assert_eq!(BYTES1.len(), len as _);
                got_writes += 1;
            }
            REQ_TYPE_READ => {
                let bufs = buf_ring.rc.get_bufs(&buf_ring, len, cqe.flags());
                assert_eq!(1, bufs.len());
                assert_eq!(Some(1), cqueue::buffer_select(cqe.flags()));
                assert_eq!(BYTES1.len(), len as _);
                assert_eq!(BYTES1, bufs[0].as_slice());
                assert!(cqueue::more(cqe.flags()));
                got_reads += 1;
                got_bufs.insert(1);
            }
            _ => {}
        }
    }
    assert_eq!(got_writes, 1);
    assert_eq!(got_reads, 1);
    assert_eq!(got_bufs, BTreeSet::from([1]));

    // Write BYTES0 and BYTES1

    unsafe {
        let queue = ring.submission();
        queue
            .push(|entry| {
                opcode::Write::new(
                    entry,
                    types::Fd(tx.as_raw_fd()),
                    BYTES0.as_ptr(),
                    BYTES0.len() as _,
                )
                .sqe_flags(squeue::Flags::IO_LINK)
                .user_data(REQ_TYPE_WRITE_BYTES0);
            })
            .expect("queue is full");
        queue
            .push(|entry| {
                opcode::Write::new(
                    entry,
                    types::Fd(tx.as_raw_fd()),
                    BYTES1.as_ptr(),
                    BYTES1.len() as _,
                )
                .sqe_flags(squeue::Flags::IO_LINK)
                .user_data(REQ_TYPE_WRITE_BYTES1);
            })
            .expect("queue is full");
    }
    ring.submit_and_wait(4)?;

    // Process two write/read pairs. Fills the first and second buffer in the ring.

    got_writes = 0;
    got_reads = 0;
    got_bufs = BTreeSet::new();
    for cqe in ring.completion().map(Into::<cqueue::Entry>::into) {
        assert!(cqe.result() >= 0);
        let len = cqe.result().cast_unsigned();
        match cqe.user_data() {
            REQ_TYPE_WRITE_BYTES0 => {
                assert_eq!(BYTES0.len(), len as _);
                assert_eq!(got_writes, 0);
                got_writes += 1;
            }
            REQ_TYPE_WRITE_BYTES1 => {
                assert_eq!(BYTES1.len(), len as _);
                assert_eq!(got_writes, 1);
                got_writes += 1;
            }
            REQ_TYPE_READ => {
                let bufs = buf_ring.rc.get_bufs(&buf_ring, len, cqe.flags());
                assert_eq!(1, bufs.len());
                match cqueue::buffer_select(cqe.flags()) {
                    Some(idx @ 0) => {
                        assert_eq!(BYTES0.len(), len as _);
                        assert_eq!(BYTES0, bufs[0].as_slice());
                        assert_eq!(got_reads, 0);
                        assert_eq!(got_bufs, BTreeSet::from([]));
                        got_bufs.insert(idx);
                    }
                    Some(idx @ 1) => {
                        assert_eq!(BYTES1.len(), len as _);
                        assert_eq!(BYTES1, bufs[0].as_slice());
                        assert_eq!(got_reads, 1);
                        assert_eq!(got_bufs, BTreeSet::from([0]));
                        got_bufs.insert(idx);
                    }
                    _ => unreachable!(),
                }
                assert!(cqueue::more(cqe.flags()));
                got_reads += 1;
            }
            _ => {}
        }
    }
    assert_eq!(got_writes, 2);
    assert_eq!(got_reads, 2);
    assert_eq!(got_bufs, BTreeSet::from([0, 1]));

    // Close the pipe writer fd to observe termination of the multi-read.

    drop(tx);

    ring.submit_and_wait(0)?;
    let mut completions = ring.completion().map(Into::<cqueue::Entry>::into);
    assert_eq!(1, completions.len());

    let cqe = completions.next().unwrap();
    assert!(cqe.result() >= 0);
    let len = cqe.result().cast_unsigned();
    assert_eq!(0, len);
    assert_eq!(REQ_TYPE_READ, cqe.user_data());
    assert!(!cqueue::more(cqe.flags()));

    Ok(())
}

pub fn test_file_writev_readv<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Writev::CODE);
        test.probe.is_supported(opcode::Readv::CODE);
    );

    println!("test file_writev_readv");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.as_raw_fd());

    utils::writev_readv(ring, fd, fd)?;

    Ok(())
}

pub fn test_pipe_fixed_writev_readv<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::WritevFixed::CODE);
        test.probe.is_supported(opcode::ReadvFixed::CODE);
    );

    println!("test pipe_fixed_writev_readv");

    const REQ_TYPE_WRITEV_FIXED: u64 = 1;
    const REQ_TYPE_READV_FIXED: u64 = 2;

    ring.submitter().unregister_buffers()?;

    let (rx, tx) = ::std::io::pipe()?;

    const BYTES0: &[u8] = "The quick brown fox jumps over the lazy dog.".as_bytes();
    const BYTES1: &[u8] = "我能吞下玻璃而不伤身体。".as_bytes();

    let mut src = Vec::with_capacity(BYTES0.len() + BYTES1.len());
    src.extend_from_slice(BYTES0);
    src.extend_from_slice(BYTES1);

    let mut dst = vec![0u8; BYTES0.len() + BYTES1.len()];

    unsafe {
        ring.submitter().register_buffers(&[
            ::libc::iovec {
                iov_base: src.as_mut_ptr() as *mut _,
                iov_len: src.len(),
            },
            ::libc::iovec {
                iov_base: dst.as_mut_ptr() as *mut _,
                iov_len: dst.len(),
            },
        ])?;
    }

    let src_parts = [
        ::libc::iovec {
            iov_base: src.as_ptr() as *mut _,
            iov_len: BYTES0.len(),
        },
        ::libc::iovec {
            iov_base: unsafe { src.as_ptr().add(BYTES0.len()) } as *mut _,
            iov_len: BYTES1.len(),
        },
    ];
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::WritevFixed::new(
                    entry,
                    types::Fd(tx.as_raw_fd()),
                    src_parts.as_ptr().cast(),
                    src_parts.len() as u32,
                    0,
                )
                .user_data(REQ_TYPE_WRITEV_FIXED);
            })
            .expect("queue is full");
    }
    ring.submit_and_wait(1)?;
    for cqe in ring.completion().map(Into::<cqueue::Entry>::into) {
        assert_eq!(cqe.user_data(), REQ_TYPE_WRITEV_FIXED);
        assert!(cqe.result() >= 0);
        let len = cqe.result();
        assert_eq!(len, src.len() as _);
    }

    let dst_parts = [
        ::libc::iovec {
            iov_base: dst.as_ptr() as *mut _,
            iov_len: BYTES0.len(),
        },
        ::libc::iovec {
            iov_base: unsafe { dst.as_ptr().add(BYTES0.len()) } as *mut _,
            iov_len: BYTES1.len(),
        },
    ];
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::ReadvFixed::new(
                    entry,
                    types::Fd(rx.as_raw_fd()),
                    dst_parts.as_ptr().cast(),
                    dst_parts.len() as u32,
                    1,
                )
                .user_data(REQ_TYPE_READV_FIXED);
            })
            .expect("queue is full");
    }
    ring.submit_and_wait(1)?;
    for cqe in ring.completion().map(Into::<cqueue::Entry>::into) {
        assert_eq!(cqe.user_data(), REQ_TYPE_READV_FIXED);
        assert!(cqe.result() >= 0);
        let len = cqe.result();
        assert_eq!(len, src.len() as _);
    }

    ring.submitter().unregister_buffers()?;

    assert_eq!(src, dst);

    Ok(())
}

pub fn test_file_fsync<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Fsync::CODE);
    );

    println!("test file_fsync");

    let mut fd = tempfile::tempfile()?;
    let n = fd.write(&[0x1])?;
    assert_eq!(n, 1);

    let fd = types::Fd(fd.as_raw_fd());

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Fsync::new(entry, fd).user_data(0x03);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x03);
    assert_eq!(cqes[0].result(), 0);

    Ok(())
}

pub fn test_file_fsync_file_range<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::SyncFileRange::CODE);
    );

    println!("test file_fsync_file_range");

    let mut fd = tempfile::tempfile()?;
    let n = fd.write(&[0x2; 3 * 1024])?;
    assert_eq!(n, 3 * 1024);
    let n = fd.write(&[0x3; 1024])?;
    assert_eq!(n, 1024);

    let fd = types::Fd(fd.as_raw_fd());

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::SyncFileRange::new(entry, fd, 1024)
                    .offset(3 * 1024)
                    .user_data(0x04);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x04);
    assert_eq!(cqes[0].result(), 0);

    Ok(())
}

/// Test `IORING_OP_FSYNC` with an optional file range (`off`/`len` on the SQE).
pub fn test_file_fsync_range<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Fsync::CODE);
    );

    println!("test file_fsync_range");

    let mut fd = tempfile::tempfile()?;
    let n = fd.write(&[0x4; 4 * 1024])?;
    assert_eq!(n, 4 * 1024);

    let fd = types::Fd(fd.as_raw_fd());

    // Sync a 1 KiB window starting at offset 3 KiB.
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Fsync::new(entry, fd)
                    .offset(3 * 1024)
                    .len(1024)
                    .user_data(0x14);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x14);
    assert_eq!(cqes[0].result(), 0);

    Ok(())
}

pub fn test_file_fallocate<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Fallocate::CODE);
    );

    println!("test file_fallocate");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.as_raw_fd());

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Fallocate::new(entry, fd, 1024).user_data(0x10);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x10);
    assert_eq!(cqes[0].result(), 0);

    Ok(())
}

pub fn test_file_openat2<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::OpenAt2::CODE);
    );

    use tempfile::tempdir;

    println!("test file_openat2");

    let dir = tempdir()?;
    let dirfd = types::Fd(libc::AT_FDCWD);

    let path = dir.path().join("test-io-uring-openat2");
    let path = CString::new(path.as_os_str().as_bytes())?;

    let openhow = types::OpenHow::new().flags(libc::O_CREAT as _);

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::OpenAt2::new(entry, dirfd, path.as_ptr(), &openhow).user_data(0x11);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x11);
    assert!(cqes[0].result() > 0);

    let fd = unsafe { fs::File::from_raw_fd(cqes[0].result()) };

    assert!(fd.metadata()?.is_file());

    Ok(())
}

pub fn test_file_openat2_close_file_index<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    // Tests close too.

    require!(
        test;
        test.probe.is_supported(opcode::OpenAt2::CODE);
        test.probe.is_supported(opcode::Close::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // to ensure fixed table support
    );

    // Cleanup all fixed files (if any), then reserve two slots.
    let _ = ring.submitter().unregister_files();
    ring.submitter().register_files_sparse(2).unwrap();

    use tempfile::tempdir;

    println!("test file_openat2_close_file_index");

    let dir = tempdir()?;
    let dirfd = types::Fd(libc::AT_FDCWD);

    // One more round than table size.
    for round in 0..3 {
        let path = dir.path().join(format!(
            "test-io-uring-openat2-file_index-a-round-{}",
            round
        ));
        let path = CString::new(path.as_os_str().as_bytes())?;

        let openhow = types::OpenHow::new().flags(libc::O_CREAT as _);

        let file_index = types::DestinationSlot::auto_target();

        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::OpenAt2::new(entry, dirfd, path.as_ptr(), &openhow)
                        .file_index(Some(file_index))
                        .user_data(0x11);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x11);
        if round == 2 {
            assert!(cqes[0].result() < 0); // expect no room
        } else {
            assert_eq!(cqes[0].result(), round); // expect auto selection to go 0, then 1.
        }
    }

    // Drop two.
    for round in 0..2 {
        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::Close::new(entry, types::Fixed(round)).user_data(0x12);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x12);
        assert_eq!(cqes[0].result(), 0); // successful close iff result is 0
    }

    // Redo the tests but with manual selection of the file_index value,
    // and reverse the order for good measure: so 2, 1, then 0.
    // Another difference: the sucessful result should be zero, not the fixed slot number since
    // we have not asked for an auto selection to be made for us.

    // One more round than table size.
    for round in (0..3).rev() {
        let path = dir.path().join(format!(
            "test-io-uring-openat2-file_index-b-round-{}",
            round
        ));
        let path = CString::new(path.as_os_str().as_bytes())?;

        let openhow = types::OpenHow::new().flags(libc::O_CREAT as _);

        let file_index = types::DestinationSlot::try_from_slot_target(round).unwrap();

        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::OpenAt2::new(entry, dirfd, path.as_ptr(), &openhow)
                        .file_index(Some(file_index))
                        .user_data(0x11);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x11);
        if round == 2 {
            assert!(cqes[0].result() < 0); // expect 2 won't fit, even though it is being asked for first.
        } else {
            assert_eq!(cqes[0].result(), 0); // success iff zero
        }
    }

    // Drop two.
    for round in 0..2 {
        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::Close::new(entry, types::Fixed(round)).user_data(0x12);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x12);
        assert_eq!(cqes[0].result(), 0); // successful close iff result is 0
    }
    // If the fixed-socket operation worked properly, this must not fail.
    ring.submitter().unregister_files().unwrap();

    Ok(())
}

// This is like the openat2 test of the same name, but uses openat instead.
pub fn test_file_openat_close_file_index<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    // Tests close too.

    require!(
        test;
        test.probe.is_supported(opcode::OpenAt::CODE);
        test.probe.is_supported(opcode::Close::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // to ensure fixed table support
    );

    // Cleanup all fixed files (if any), then reserve two slots.
    let _ = ring.submitter().unregister_files();
    ring.submitter().register_files_sparse(2).unwrap();

    use tempfile::tempdir;

    println!("test file_openat_close_file_index");

    let dir = tempdir()?;
    let dirfd = types::Fd(libc::AT_FDCWD);

    // One more round than table size.
    for round in 0..3 {
        let path = dir
            .path()
            .join(format!("test-io-uring-openat-file_index-a-round-{}", round));
        let path = CString::new(path.as_os_str().as_bytes())?;

        let file_index = types::DestinationSlot::auto_target();

        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::OpenAt::new(entry, dirfd, path.as_ptr())
                        .flags(libc::O_CREAT as _)
                        .file_index(Some(file_index))
                        .user_data(0x11);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x11);
        if round == 2 {
            assert!(cqes[0].result() < 0); // expect no room
        } else {
            assert_eq!(cqes[0].result(), round); // expect auto selection to go 0, then 1.
        }
    }

    // Drop two.
    for round in 0..2 {
        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::Close::new(entry, types::Fixed(round)).user_data(0x12);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x12);
        assert_eq!(cqes[0].result(), 0); // successful close iff result is 0
    }

    // Redo the tests but with manual selection of the file_index value,
    // and reverse the order for good measure: so 2, 1, then 0.
    // Another difference: the sucessful result should be zero, not the fixed slot number since
    // we have not asked for an auto selection to be made for us.

    // One more round than table size.
    for round in (0..3).rev() {
        let path = dir
            .path()
            .join(format!("test-io-uring-openat-file_index-b-round-{}", round));
        let path = CString::new(path.as_os_str().as_bytes())?;

        let file_index = types::DestinationSlot::try_from_slot_target(round).unwrap();

        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::OpenAt::new(entry, dirfd, path.as_ptr())
                        .flags(libc::O_CREAT as _)
                        .file_index(Some(file_index))
                        .user_data(0x11);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x11);
        if round == 2 {
            assert!(cqes[0].result() < 0); // expect 2 won't fit, even though it is being asked for first.
        } else {
            assert_eq!(cqes[0].result(), 0); // success iff zero
        }
    }

    // Drop two.
    for round in 0..2 {
        unsafe {
            ring.submission()
                .push(|entry| {
                    opcode::Close::new(entry, types::Fixed(round)).user_data(0x12);
                })
                .expect("queue is full");
        }

        ring.submit_and_wait(1)?;

        let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 0x12);
        assert_eq!(cqes[0].result(), 0); // successful close iff result is 0
    }
    // If the fixed-socket operation worked properly, this must not fail.
    ring.submitter().unregister_files().unwrap();

    Ok(())
}

pub fn test_file_close<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Close::CODE);
    );

    println!("test file_cloes");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.into_raw_fd());

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Close::new(entry, fd).user_data(0x12);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x12);
    assert_eq!(cqes[0].result(), 0);

    Ok(())
}

pub fn test_file_cur_pos<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Write::CODE);
        test.probe.is_supported(opcode::Read::CODE);
        ring.params().is_feature_rw_cur_pos();
    );

    println!("test file_cur_pos");

    let fd = tempfile::tempfile()?;
    let fd = types::Fd(fd.into_raw_fd());

    let text = b"The quick brown fox jumps over the lazy dog.";
    let mut output = vec![0; text.len()];

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Write::new(entry, fd, text.as_ptr(), 22)
                    .offset(u64::MAX)
                    .user_data(0x01);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Write::new(entry, fd, text.as_ptr().add(22), 22)
                    .offset(u64::MAX)
                    .user_data(0x02);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(2)?;

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Read::new(entry, fd, output.as_mut_ptr(), output.len() as _)
                    .user_data(0x03);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(3)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 3);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[1].user_data(), 0x02);
    assert_eq!(cqes[2].user_data(), 0x03);
    assert_eq!(cqes[0].result(), 22);
    assert_eq!(cqes[1].result(), 22);
    assert_eq!(cqes[2].result(), text.len() as i32);

    assert_eq!(&output, text);

    Ok(())
}

#[cfg(not(feature = "ci"))]
pub fn test_statx<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Statx::CODE);
    );

    println!("test statx");

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test-io-uring-statx");
    let pathbuf = CString::new(path.as_os_str().as_bytes())?;
    fs::write(&path, "1")?;

    let mut statxbuf: libc::statx = unsafe { std::mem::zeroed() };

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Statx::new(
                    entry,
                    types::Fd(libc::AT_FDCWD),
                    pathbuf.as_ptr(),
                    &mut statxbuf as *mut libc::statx as *mut _,
                )
                .mask(libc::STATX_ALL)
                .user_data(0x99);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x99);
    assert_eq!(cqes[0].result(), 0);

    // check
    let mut statxbuf2 = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::statx(
            libc::AT_FDCWD,
            pathbuf.as_ptr(),
            0,
            libc::STATX_ALL,
            &mut statxbuf2,
        )
    };

    assert_eq!(ret, 0);
    assert_eq!(statxbuf, statxbuf2);

    // statx fd
    let fd = fs::File::open(&path)?;
    let mut statxbuf3: libc::statx = unsafe { std::mem::zeroed() };

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Statx::new(
                    entry,
                    types::Fd(fd.as_raw_fd()),
                    b"\0".as_ptr().cast(),
                    &mut statxbuf3 as *mut libc::statx as *mut _,
                )
                .flags(libc::AT_EMPTY_PATH)
                .mask(libc::STATX_ALL)
                .user_data(0x9a);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x9a);
    assert_eq!(cqes[0].result(), 0);

    assert_eq!(statxbuf3, statxbuf2);

    Ok(())
}

pub fn test_file_direct_write_read<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use tempfile::TempDir;

    #[repr(align(4096))]
    struct AlignedBuffer([u8; 4096]);

    require!(
        test;
        test.probe.is_supported(opcode::Write::CODE);
        test.probe.is_supported(opcode::Read::CODE);
    );

    println!("test file_direct_write_read");

    let dir = TempDir::new_in(".")?;
    let fd = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_DIRECT)
        .open(dir.path().join("io-uring-test-file"))?;
    let fd = types::Fd(fd.as_raw_fd());

    // ok

    let input = Box::new(AlignedBuffer([0xf9; 4096]));
    let mut output = Box::new(AlignedBuffer([0x0; 4096]));

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Write::new(entry, fd, input.0.as_ptr(), input.0.len() as _).user_data(0x01);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(1)?, 1);

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Read::new(entry, fd, output.0.as_mut_ptr(), output.0.len() as _)
                    .user_data(0x02);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(2)?, 1);

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 2);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[1].user_data(), 0x02);
    assert_eq!(cqes[0].result(), input.0.len() as i32);
    assert_eq!(cqes[1].result(), input.0.len() as i32);

    assert_eq!(input.0[..], output.0[..]);
    assert_eq!(input.0[0], 0xf9);

    // fail

    let mut buf = Box::new(AlignedBuffer([0; 4096]));
    let buf = &mut buf.0;

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Read::new(entry, fd, buf[1..].as_mut_ptr(), buf[1..].len() as _)
                    .user_data(0x03);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(1)?, 1);

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x03);

    // when fs does not support Direct IO, it may fallback to buffered IO.
    let res = cqes[0].result();
    assert!(
        res == -libc::EINVAL || res == buf[1..].len() as i32,
        "unexpected misaligned O_DIRECT read result: {}",
        res
    );

    Ok(())
}

pub fn test_file_splice<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    use std::io::Read;

    require!(
        test;
        test.probe.is_supported(opcode::Splice::CODE);
    );

    println!("test file_splice");

    let dir = tempfile::TempDir::new_in(".")?;
    let dir = dir.path();

    let input = &[0x9f; 1024];

    let (pipe_in, mut pipe_out) = {
        let mut pipes = [0, 0];
        let ret = unsafe { libc::pipe(pipes.as_mut_ptr()) };
        assert_eq!(ret, 0);
        let pipe_out = unsafe { fs::File::from_raw_fd(pipes[0]) };
        let pipe_in = unsafe { fs::File::from_raw_fd(pipes[1]) };
        (pipe_in, pipe_out)
    };

    fs::write(dir.join("io-uring-test-file-input"), input)?;
    let fd = fs::File::open(dir.join("io-uring-test-file-input"))?;

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Splice::new(
                    entry,
                    types::Fd(fd.as_raw_fd()),
                    0,
                    types::Fd(pipe_in.as_raw_fd()),
                    -1,
                    1024,
                )
                .user_data(0x33);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x33);
    assert_eq!(cqes[0].result(), 1024);

    let mut output = [0; 1024];
    pipe_out.read_exact(&mut output)?;

    assert_eq!(input, &output[..]);

    Ok(())
}

pub fn test_ftruncate<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Ftruncate::CODE);
    );

    println!("test ftruncate");

    let dir = tempfile::TempDir::new_in(".")?;
    let dir = dir.path();
    let file = dir.join("io-uring-test-file-input");

    let input = &[0x9f; 1024];

    fs::write(&file, input)?;
    let fd = fs::OpenOptions::new().write(true).open(&file)?;
    let fd = types::Fd(fd.as_raw_fd());

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Ftruncate::new(entry, fd, 512).user_data(0x33);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x33);
    assert_eq!(cqes[0].result(), 0);
    assert_eq!(
        fs::read(&file).expect("could not read truncated file"),
        &input[..512]
    );

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Ftruncate::new(entry, fd, 0).user_data(0x34);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x34);
    assert_eq!(cqes[0].result(), 0);
    assert_eq!(
        fs::metadata(&file)
            .expect("could not read truncated file")
            .len(),
        0
    );

    Ok(())
}

pub fn test_fixed_fd_install<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Read::CODE);
        test.probe.is_supported(opcode::FixedFdInstall::CODE);
    );

    println!("test fixed_fd_install");

    let dir = tempfile::TempDir::new_in(".")?;
    let dir = dir.path();
    let file = dir.join("io-uring-test-file-input");

    let input = &[0x9f; 1024];
    let mut output = vec![0; 1024];

    fs::write(&file, input)?;
    let fd = fs::OpenOptions::new().read(true).open(&file)?;
    let fd = types::Fd(fd.as_raw_fd());
    ring.submitter().register_files(&[fd.0])?;
    let fd = types::Fixed(0);

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Read::new(entry, fd, output.as_mut_ptr(), output.len() as _)
                    .user_data(0x01);
            })
            .expect("queue is full");
    }

    assert_eq!(ring.submit_and_wait(1)?, 1);
    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[0].result(), 1024);
    assert_eq!(output, input);

    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::FixedFdInstall::new(entry, fd, 0).user_data(0x02);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();

    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x02);
    let fd = cqes[0].result();
    assert!(fd > 0);
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    file.read_exact(&mut output)?;
    assert_eq!(output, input);

    Ok(())
}

pub fn test_get_set_xattr<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::GetXattr::CODE);
        test.probe.is_supported(opcode::SetXattr::CODE);
    );

    println!("test get_set_xattr");

    let dir = tempfile::tempdir()?;
    let file_path = dir.path().join("test-file");
    fs::write(&file_path, b"test content")?;

    let file_path_cstr = CString::new(file_path.as_os_str().as_bytes())?;

    let attr_name = CString::new("user.test_attr")?;
    let attr_value = CString::new("test_value")?;
    let mut buffer = vec![0u8; 128];

    // Set extended attribute
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::SetXattr::new(
                    entry,
                    attr_name.as_ptr(),
                    attr_value.as_ptr().cast(),
                    file_path_cstr.as_ptr().cast(),
                    attr_value.as_bytes().len() as u32,
                )
                .flags(0)
                .user_data(0x01);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[0].result(), 0);

    // Get extended attribute
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::GetXattr::new(
                    entry,
                    attr_name.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    file_path_cstr.as_ptr().cast(),
                    buffer.len() as u32,
                )
                .user_data(0x02);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x02);
    assert_eq!(cqes[0].result(), attr_value.as_bytes().len() as i32);

    let retrieved_value = CString::new(&buffer[..cqes[0].result() as usize])?;
    assert_eq!(retrieved_value, attr_value);

    Ok(())
}

pub fn test_f_get_set_xattr<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::FGetXattr::CODE);
        test.probe.is_supported(opcode::FSetXattr::CODE);
    );

    println!("test f_get_set_xattr");

    let file = tempfile::tempfile()?;
    let fd = types::Fd(file.as_raw_fd());

    let attr_name = CString::new("user.test_attr")?;
    let attr_value = CString::new("test_value")?;
    let mut buffer = vec![0u8; 128];

    // Set extended attribute on file descriptor
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::FSetXattr::new(
                    entry,
                    fd,
                    attr_name.as_ptr(),
                    attr_value.as_ptr().cast(),
                    attr_value.as_bytes().len() as u32,
                )
                .flags(0)
                .user_data(0x01);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x01);
    assert_eq!(cqes[0].result(), 0);

    // Get extended attribute from file descriptor
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::FGetXattr::new(
                    entry,
                    fd,
                    attr_name.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                )
                .user_data(0x02);
            })
            .expect("queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    assert_eq!(cqes.len(), 1);
    assert_eq!(cqes[0].user_data(), 0x02);
    assert_eq!(cqes[0].result(), attr_value.as_bytes().len() as i32);

    let retrieved_value = CString::new(&buffer[..cqes[0].result() as usize])?;
    assert_eq!(retrieved_value, attr_value);

    Ok(())
}

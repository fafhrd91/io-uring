use crate::Test;
use ntex_io_uring::types::CancelBuilder;
use ntex_io_uring::{cqueue, opcode, squeue, types, IoUring};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::io::AsRawFd;

// Cancels one request matching the user data.
pub fn test_async_cancel_user_data<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Timeout::CODE);
        test.probe.is_supported(opcode::AsyncCancel2::CODE);
    );

    println!("test async_cancel_user_data");

    let ts = types::Timespec::new().sec(1);
    let mut timeout_e = squeue::Entry::default();
    opcode::Timeout::new(&mut timeout_e, &ts).user_data(2003);

    // Cancel the timeout matching user data
    let builder = CancelBuilder::user_data(2003);
    let mut cancel_e = squeue::Entry::default();
    opcode::AsyncCancel2::new(&mut cancel_e, builder).user_data(2004);

    let entries = [timeout_e, cancel_e];
    for sqe in &entries {
        unsafe {
            ring.submission()
                .push(|e| *e = S::from(sqe.clone()))
                .expect("queue is full");
        }
    }

    // Wait for both timeout and the cancel request.
    ring.submit_and_wait(entries.len())?;

    let mut cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    cqes.sort_unstable_by_key(cqueue::Entry::user_data);

    assert_eq!(cqes.len(), entries.len());

    assert_eq!(cqes[0].user_data(), 2003);
    assert_eq!(cqes[1].user_data(), 2004);

    assert_eq!(cqes[0].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[1].result(), 0); // the number of requests cancelled

    Ok(())
}

// Cancels one request matching the user data.
pub fn test_async_cancel_user_data_all<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Timeout::CODE);
        test.probe.is_supported(opcode::AsyncCancel2::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // Check if Kernel >= 5.19
    );

    println!("test async_cancel_user_data_all");

    let ts = types::Timespec::new().sec(1);
    let mut timeout_e = squeue::Entry::default();
    opcode::Timeout::new(&mut timeout_e, &ts).user_data(2003);

    // Cancel all timeouts matching user data
    let builder = CancelBuilder::user_data(2003).all();
    let mut cancel_e = squeue::Entry::default();
    opcode::AsyncCancel2::new(&mut cancel_e, builder).user_data(2004);

    let entries = [timeout_e.clone(), timeout_e, cancel_e];
    for sqe in &entries {
        unsafe {
            ring.submission()
                .push(|e| *e = S::from(sqe.clone()))
                .expect("queue is full");
        }
    }

    // Wait for both timeouts and the cancel request.
    ring.submit_and_wait(entries.len())?;

    let mut cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    cqes.sort_unstable_by_key(cqueue::Entry::user_data);

    assert_eq!(cqes.len(), entries.len());

    assert_eq!(cqes[0].user_data(), 2003);
    assert_eq!(cqes[1].user_data(), 2003);
    assert_eq!(cqes[2].user_data(), 2004);

    assert_eq!(cqes[0].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[1].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[2].result(), 2); // the number of requests cancelled

    Ok(())
}

// Cancels any requests.
pub fn test_async_cancel_any<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::Timeout::CODE);
        test.probe.is_supported(opcode::AsyncCancel2::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // Check if Kernel >= 5.19
    );

    println!("test async_cancel_any");

    let ts = types::Timespec::new().sec(1);
    let mut timeout_e = squeue::Entry::default();
    opcode::Timeout::new(&mut timeout_e, &ts).user_data(2003);
    let mut timeout_e2 = timeout_e.clone();
    timeout_e2.set_user_data(2004);

    // Cancel any pending requests
    let builder = CancelBuilder::any();
    let mut cancel_e = squeue::Entry::default();
    opcode::AsyncCancel2::new(&mut cancel_e, builder).user_data(2005);

    let entries = [timeout_e, timeout_e2, cancel_e];
    for sqe in &entries {
        unsafe {
            ring.submission()
                .push(|e| *e = S::from(sqe.clone()))
                .expect("queue is full");
        }
    }

    // Wait for both timeouts and the cancel request.
    ring.submit_and_wait(entries.len())?;

    let mut cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    cqes.sort_unstable_by_key(cqueue::Entry::user_data);

    assert_eq!(cqes.len(), entries.len());

    assert_eq!(cqes[0].user_data(), 2003);
    assert_eq!(cqes[1].user_data(), 2004);
    assert_eq!(cqes[2].user_data(), 2005);

    assert_eq!(cqes[0].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[1].result(), -libc::ECANCELED);
    assert_eq!(cqes[2].result(), 2); // the number of requests cancelled

    Ok(())
}

pub fn test_async_cancel_fd<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::PollAdd::CODE);
        test.probe.is_supported(opcode::AsyncCancel2::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // Check if Kernel >= 5.19
    );

    println!("test async_cancel_fd");

    let _fd = create_dummy_fd()?;
    let fd = types::Fd(_fd.as_raw_fd());
    let mut poll_e = squeue::Entry::default();
    opcode::PollAdd::new(&mut poll_e, fd, libc::POLLIN as _).user_data(2003);

    // Cancel one poll request matching FD
    let builder = CancelBuilder::fd(fd);
    let mut cancel_e = squeue::Entry::default();
    opcode::AsyncCancel2::new(&mut cancel_e, builder).user_data(2004);

    let entries = [poll_e, cancel_e];
    for sqe in &entries {
        unsafe {
            ring.submission()
                .push(|e| *e = S::from(sqe.clone()))
                .expect("queue is full");
        }
    }

    // Wait for 2 requests: canceled poll and the cancel request itself.
    ring.submit_and_wait(entries.len())?;

    let mut cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    cqes.sort_unstable_by_key(cqueue::Entry::user_data);

    assert_eq!(cqes.len(), entries.len());

    assert_eq!(cqes[0].user_data(), 2003);
    assert_eq!(cqes[1].user_data(), 2004);

    assert_eq!(cqes[0].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[1].result(), 0);

    Ok(())
}

// Cancels all pending requests with the given FD.
pub fn test_async_cancel_fd_all<S: squeue::EntryMarker, C: cqueue::EntryMarker>(
    ring: &mut IoUring<S, C>,
    test: &Test,
) -> anyhow::Result<()> {
    require!(
        test;
        test.probe.is_supported(opcode::PollAdd::CODE);
        test.probe.is_supported(opcode::AsyncCancel2::CODE);
        test.probe.is_supported(opcode::Socket::CODE); // Check if Kernel >= 5.19
    );

    println!("test async_cancel_fd_all");

    let _fd = create_dummy_fd()?;
    let fd = types::Fd(_fd.as_raw_fd());
    let mut poll_e = squeue::Entry::default();
    opcode::PollAdd::new(&mut poll_e, fd, libc::POLLIN as _).user_data(2003);
    let mut poll_e2 = poll_e.clone();
    poll_e2.set_user_data(2004);

    // Cancel all requests matching FD
    let builder = CancelBuilder::fd(fd).all();
    let mut cancel_e = squeue::Entry::default();
    opcode::AsyncCancel2::new(&mut cancel_e, builder).user_data(2005);

    let entries = [poll_e, poll_e2, cancel_e];
    for sqe in &entries {
        unsafe {
            ring.submission()
                .push(|e| *e = S::from(sqe.clone()))
                .expect("queue is full");
        }
    }

    // Wait for both polls and the cancel request.
    ring.submit_and_wait(entries.len())?;

    let mut cqes: Vec<cqueue::Entry> = ring.completion().map(Into::into).collect();
    cqes.sort_unstable_by_key(cqueue::Entry::user_data);

    assert_eq!(cqes.len(), entries.len());

    assert_eq!(cqes[0].user_data(), 2003);
    assert_eq!(cqes[1].user_data(), 2004);
    assert_eq!(cqes[2].user_data(), 2005);

    assert_eq!(cqes[0].result(), -libc::ECANCELED); // -ECANCELED
    assert_eq!(cqes[1].result(), -libc::ECANCELED);
    assert_eq!(cqes[2].result(), 2); // the number of requests cancelled

    Ok(())
}

fn create_dummy_fd() -> anyhow::Result<File> {
    unsafe {
        let fd = libc::eventfd(0, libc::EFD_CLOEXEC);

        if fd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }

        Ok(File::from_raw_fd(fd))
    }
}

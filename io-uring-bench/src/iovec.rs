use std::io;
use std::os::unix::io::AsRawFd;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ntex_io_uring::{opcode, squeue, types, IoUring};
use tempfile::tempfile;

fn bench_iovec(c: &mut Criterion) {
    let mut ring = IoUring::new(16).unwrap();

    let buf = vec![0x00; 32];
    let buf2 = vec![0x01; 33];
    let buf3 = vec![0x02; 34];
    let buf4 = vec![0x03; 35];
    let buf5 = vec![0x04; 36];

    c.bench_function("writev-one", |b| {
        let fd = tempfile().unwrap();

        let bufs = [
            io::IoSlice::new(&buf),
            io::IoSlice::new(&buf2),
            io::IoSlice::new(&buf3),
            io::IoSlice::new(&buf4),
            io::IoSlice::new(&buf5),
        ];

        b.iter(|| {
            let bufs = black_box(&bufs);

            unsafe {
                ring.submission()
                    .push(|entry| {
                        opcode::Writev::new(
                            entry,
                            types::Fd(fd.as_raw_fd()),
                            bufs.as_ptr() as *const _,
                            bufs.len() as _,
                        );
                    })
                    .ok()
                    .expect("queue is full");
            }

            ring.submit_and_wait(1).unwrap();
            ring.completion().map(black_box).for_each(drop);
        });
    });

    // nop so slow :(
    c.bench_function("writev-nop", |b| {
        let fd = tempfile().unwrap();

        let bufs = [
            io::IoSlice::new(&buf),
            io::IoSlice::new(&buf2),
            io::IoSlice::new(&buf3),
            io::IoSlice::new(&buf4),
            io::IoSlice::new(&buf5),
        ];

        b.iter(|| {
            let bufs = black_box(&bufs);

            unsafe {
                let queue = ring.submission();
                queue
                    .push(|entry| {
                        opcode::Writev::new(
                            entry,
                            types::Fd(fd.as_raw_fd()),
                            bufs.as_ptr() as *const _,
                            bufs.len() as _,
                        )
                        .sqe_flags(squeue::Flags::IO_LINK);
                    })
                    .ok()
                    .expect("queue is full");
                for _ in 0..4 {
                    queue
                        .push(|entry| {
                            opcode::Nop::new(entry).sqe_flags(squeue::Flags::IO_LINK);
                        })
                        .ok()
                        .expect("queue is full");
                }
            }

            ring.submit_and_wait(5).unwrap();
            ring.completion().map(black_box).for_each(drop);
        });
    });

    c.bench_function("writes-one-submit", |b| {
        let fd = tempfile().unwrap();

        let bufs = [&buf[..], &buf2[..], &buf3[..], &buf4[..], &buf5[..]];

        b.iter(|| {
            let queue = ring.submission();
            for buf in black_box(&bufs) {
                unsafe {
                    queue
                        .push(|entry| {
                            opcode::Write::new(
                                entry,
                                types::Fd(fd.as_raw_fd()),
                                buf.as_ptr(),
                                buf.len() as _,
                            )
                            .sqe_flags(squeue::Flags::IO_LINK);
                        })
                        .ok()
                        .expect("queue is full");
                }
            }

            drop(queue);

            ring.submit_and_wait(bufs.len()).unwrap();
            ring.completion().map(black_box).for_each(drop);
        });
    });

    c.bench_function("writes-multi-submit", |b| {
        let fd = tempfile().unwrap();

        let bufs = [&buf[..], &buf2[..], &buf3[..], &buf4[..], &buf5[..]];

        b.iter(|| {
            for buf in black_box(&bufs) {
                unsafe {
                    ring.submission()
                        .push(|entry| {
                            opcode::Write::new(
                                entry,
                                types::Fd(fd.as_raw_fd()),
                                buf.as_ptr(),
                                buf.len() as _,
                            );
                        })
                        .ok()
                        .expect("queue is full");
                }

                ring.submit_and_wait(1).unwrap();
                ring.completion().map(black_box).for_each(drop);
            }
        });
    });
}

criterion_group!(iovec, bench_iovec);
criterion_main!(iovec);

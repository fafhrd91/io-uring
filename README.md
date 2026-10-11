# ntex-io-uring

[![CI](https://github.com/fafhrd91/io-uring/actions/workflows/ci.yml/badge.svg)](https://github.com/fafhrd91/io-uring/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/fafhrd91/io-uring/graph/badge.svg)](https://codecov.io/gh/fafhrd91/io-uring)
[![crates.io](https://img.shields.io/crates/v/ntex-io-uring.svg)](https://crates.io/crates/ntex-io-uring)
[![Documentation](https://img.shields.io/docsrs/ntex-io-uring/latest)](https://docs.rs/ntex-io-uring)
[![MSRV](https://img.shields.io/badge/rustc-1.97+-lightgray.svg)](https://releases.rs/docs/1.97.0/)
[![License](https://img.shields.io/crates/l/ntex-io-uring.svg)](https://github.com/fafhrd91/io-uring#license)

The low-level [`io_uring`](https://kernel.dk/io_uring.pdf) userspace interface for Rust,
used by the [ntex](https://github.com/ntex-rs/ntex) runtime.

This is a fork of [tokio-rs/io-uring](https://github.com/tokio-rs/io-uring).
Main differences:

* Submission entries are initialized in place: `SubmissionQueue::push` takes a closure
  that receives the `squeue::Entry` and fills it with `opcode::X::new(entry, ..)`.
  No intermediate entry is built and copied into the ring.
* Buffer selection is enabled with a single `buffer_select(group)` call.
* Rust 2024 edition, minimum supported Rust version is 1.97.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
ntex-io-uring = "1"
```

The following is a quick introduction that reads a file with `Read`:

```rust
use ntex_io_uring::{opcode, types, IoUring};
use std::os::unix::io::AsRawFd;
use std::{fs, io};

fn main() -> io::Result<()> {
    let mut ring = IoUring::new(8)?;

    let fd = fs::File::open("README.md")?;
    let mut buf = vec![0; 1024];

    // Note that the developer needs to ensure
    // that the entry pushed into submission queue is valid (e.g. fd, buffer).
    unsafe {
        ring.submission()
            .push(|entry| {
                opcode::Read::new(
                    entry,
                    types::Fd(fd.as_raw_fd()),
                    buf.as_mut_ptr(),
                    buf.len() as _,
                )
                .user_data(0x42);
            })
            .expect("submission queue is full");
    }

    ring.submit_and_wait(1)?;

    let cqe = ring.completion().next().expect("completion queue is empty");

    assert_eq!(cqe.user_data(), 0x42);
    assert!(cqe.result() >= 0, "read error: {}", cqe.result());

    Ok(())
}
```

Note that the `Read` opcode requires Linux 5.6 or later.
Use `Probe` to check which opcodes the running kernel supports.

## Test and Benchmarks

Run the unit tests, the kernel integration tests and the benchmarks with:

```
$ cargo test
$ cargo run --package io-uring-test
$ cargo bench --package io-uring-bench
```

The integration tests need a kernel with io_uring enabled. In Docker, run the container
with `--security-opt seccomp=unconfined`.

## License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

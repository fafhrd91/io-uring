# Changes

## [1.0.0] - 2026-10-11

* opcodes initialize the submission entry in place. `X::new(&mut entry, ..)`
  resets the entry and writes required fields, setters update it directly, `build()`
  is removed. Entries can be reused for another operation. `new` takes
  `&mut impl squeue::EntryMarker` and resets all 128 bytes of an `Entry128`

* `SubmissionQueue::push()` takes a closure that initializes the queue slot
  in place with an opcode. If the queue is full it returns `PushError<F>`,
  which holds the closure. `push_multiple()`, `push_unchecked()` and
  `try_push_inline()` are removed, `has_space()` takes no arguments

* `EntryMarker` requires `AsMut<Entry>`

* `buf_group()` on `Read`, `Readv`, `Recv` and `RecvMsg` is replaced by
  `buffer_select()`, which also sets the `BUFFER_SELECT` flag

* `MsgRingSendFd::opcode_flags()` ORs flags like other flag setters

* `squeue::Entry::get_opcode()` returns `u8`

* `squeue::Entry` and `Entry128` by-value builders `flags()`, `user_data()` and
  `personality()` are replaced by in-place `set_flags()`, `set_user_data()` and
  `set_personality()`, `clear_flags()` takes `&mut self`. Added `get_flags()`
  and `get_personality()`

* sys bindings regenerated from Linux 7.2 uapi headers

* `EpollCtl::new()` takes a regular `epfd`, the kernel doesn't support fixed epoll files

* `Fadvise` and `Madvise` pass lengths larger than `u32` in the 64-bit field
  instead of truncating them

* `WaitId::infop()` takes `*mut siginfo_t`

* `Submitter::register_ifq()` takes `&mut io_uring_zcrx_ifq_reg`, the kernel writes
  results back into it

* Remove deprecated `IoUring::submission_shared()`

* Document all opcode constructors and setters, fix opcode docs

* Fix `opcode::ReadMulti` clearing `FIXED_FILE` for fixed file descriptors

* `Timeout`, `LinkTimeout` and `TimeoutUpdate` `flags()` OR flags, `PollAdd::multi()`
  keeps other poll flags

* `MsgRingData` and `MsgRingSendFd` accept a fixed (registered) ring fd

* `CancelBuilder` can match by opcode and combine user_data, fd and opcode
  criteria: `CancelBuilder::opcode()`, `match_user_data()`, `match_fd()`, `match_opcode()`

* Add `Builder::setup_sqe_mixed()`, `Parameters::is_setup_sqe_mixed()` and
  `SubmissionQueue::push128()` for 128-byte entries in a 64-byte queue, add
  `opcode::Nop128` and `opcode::UringCmd128`

* Add missing op setters: `Nop` `inject_result()`, `file()`, `buf_index()`,
  `task_work()`, `cqe32()`; `PollAdd::level()`; `nanos()` and
  `TimeoutFlags::IMMEDIATE_ARG` for timeouts; `ioprio()` for `Accept`,
  `AcceptMulti`, `RecvMulti`, `SendBundle`, `RecvBundle`, `RecvMultiBundle`;
  `AcceptMulti::addr()`; `len()` for `RecvBundle` and `RecvMultiBundle`;
  `total_len()` for `RecvMulti` and `RecvMultiBundle`; `buf_index()` for `Send`,
  `Recv` and `SendMsgZc`; `vectorized()` for `Send` and `SendZc`; `pi_attr()` for
  read/write ops; `flags()` for `UringCmd16` and `UringCmd80`. Export `types::io_uring_attr_pi`

## [0.7.150] - Unreleased

* Add `IoUring::submission_unsynced()`, returns the submission queue without
  synchronizing it with the kernel

* Add `SubmissionQueue::try_push_inline()`, returns the closure if the queue
  is full

* Breaking: `IoUring` is no longer `Send` or `Sync`, the ring uses unsynchronized
  local state and is not safe to use from multiple threads

* Deprecate `IoUring::submission_shared()`, use `IoUring::submission()` instead

* Fix rings hanging when queued entries are not explicitly synced before
  `submit()`

* Fix stale entries not being cleared when the whole ring is consumed at once

* Fix fixed-file `opcode2` operations setting the wrong SQE flag

* Run pending task work (`IORING_ENTER_GETEVENTS`) during submission when the
  kernel sets `IORING_SQ_TASKRUN`, requires `setup_taskrun_flag()`

* Debug builds panic when a `push_inline()` closure pushes to the same queue

* Optimize submission queue push path, avoid redundant tail stores and syncs

* `CompletionQueue::sync()` and drop skip the head store if no entries were consumed

* `Submitter::submit*()` compute the pending count from the local SQ tail instead of
  reloading the shared tail

* Add `opcode2::Writev` with `offset()` support

* Add `register_napi()`, `register_ring_fd()`, `register_buffers_clone()`,
  `CompletionQueue::status()` and eventfd enable/disable support

* The registered ring fd is stored in `IoUring` and shared by all its `Submitter`s, so
  `IoUring::submit()` and later `submitter()` calls use it; it is unregistered on drop.
  `register_ring_fd()` and `unregister_ring_fd()` take `&self`

* Add `setup_no_sqarray()` and `Parameters::is_feature_no_iowait()`

* Add `write_stream` support to write opcodes and range support to `Fsync`

* Tests, benches and examples use the `ntex_io_uring` crate name

# Changes

## [1.0.0] - 2026-10-11

* opcodes initialize the submission entry in place. `X::new(&mut entry, ..)`
  writes required fields into a zeroed entry, setters update it directly, `build()`
  is removed

* `SubmissionQueue::push()` takes a closure that initializes the zeroed
  queue slot in place. `push_multiple()`, `push_unchecked()`, `try_push_inline()`
  and `PushError` are removed, `has_space()` takes no arguments

* `EntryMarker` requires `AsMut<Entry>`

* `buf_group()` on `Read`, `Readv`, `Recv` and `RecvMsg` is replaced by
  `buffer_select()`, which also sets the `BUFFER_SELECT` flag

* `MsgRingSendFd::opcode_flags()` ORs flags like other flag setters

* `squeue::Entry::get_opcode()` returns `u8`

* Remove deprecated `IoUring::submission_shared()`

* Document all opcode constructors and setters, fix opcode docs

* Fix `opcode::ReadMulti` clearing `FIXED_FILE` for fixed file descriptors

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

* Add `opcode2::Writev` with `offset()` support

* Add `register_napi()`, `register_ring_fd()`, `register_buffers_clone()`,
  `CompletionQueue::status()` and eventfd enable/disable support

* Add `setup_no_sqarray()` and `Parameters::is_feature_no_iowait()`

* Add `write_stream` support to write opcodes and range support to `Fsync`

* Tests, benches and examples use the `ntex_io_uring` crate name

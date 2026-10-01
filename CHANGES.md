# Changes

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

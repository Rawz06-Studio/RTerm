# Patched dependencies

- `xattr` 0.2.3, pulled in by GPUI's HTTP client. It no longer builds on
  Linux with recent versions of `libc`, which dropped `ENOATTR` there (the
  error is `ENODATA`, the same value). The only change is in
  `src/sys/mod.rs` (plus a line silencing deprecation warnings in
  `src/lib.rs`). Remove this copy once GPUI depends on `xattr` 1.x.

# Tests

Integration tests that exercise the crate as a whole (each `tests/*.rs` file is
compiled as its own crate and can only reach the crate's public API).

Suggested scope:

- End-to-end scenarios that span multiple modules through the public API.
- Behavior that unit tests inside `src/` can't cover from outside the crate.

Tip: to test internals that aren't exposed publicly, keep those as unit tests in
`#[cfg(test)]` modules under `src/` instead.

Run with `cargo nextest run` (or `cargo test`).

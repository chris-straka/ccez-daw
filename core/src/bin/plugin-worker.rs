//! Plugin sandbox worker: the child side of `ccez_core::plugins`.
//!
//! One line in, one line out (JSON over stdio). See
//! [`ccez_core::plugins::worker`] for the protocol.

fn main() {
    std::process::exit(ccez_core::plugins::worker::worker_main());
}

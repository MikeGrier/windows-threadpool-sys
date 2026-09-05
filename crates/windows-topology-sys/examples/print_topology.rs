// Copyright (c) 2026 Mike Grier
//! Print the host's topology as JSON.
//!
//! This is the thing a consumer of this crate will want first: a description
//! of the machine it is running on, in the same shape a hand-written or
//! fed-in description takes. Redirect the output to a file and edit it by
//! hand to build a synthetic topology for a machine you do not have.
//!
//! ```text
//! cargo run --example print_topology --features serde
//! ```

use windows_topology_sys::MachineMemoryTopology;

fn main() {
    let topology = MachineMemoryTopology::discover().expect("discover the host topology");
    let json = serde_json::to_string_pretty(&topology).expect("serialize");
    println!("{json}");
}

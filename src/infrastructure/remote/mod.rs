//! Remote machines, the parts both the daemon and the CLI use
//! (`docs/remote-machines.md`, "Trust" and "Wire protocol"): the files under
//! `<data dir>/remote/`, the Noise handshake and transport, the pairing
//! commitment and code, and the JSON frames on the relay and inside
//! sessions; pairing over SSH's line protocol and `ssh` command; and on a
//! target, the control executor and its request entries.
//! The relay link itself runs in the daemon.

pub mod control;
pub mod noise;
pub mod pairing;
pub mod requests;
pub mod ssh_pair;
pub mod store;
pub mod wire;

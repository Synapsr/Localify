//! Remote machines, the parts both the daemon and the CLI use
//! (`docs/remote-machines.md`, "Trust" and "Wire protocol"): the files under
//! `<data dir>/remote/`, the Noise handshake and transport, the pairing
//! commitment and code, and the JSON frames on the relay and inside
//! sessions. The relay link itself runs in the daemon.

pub mod noise;
pub mod pairing;
pub mod store;
pub mod wire;

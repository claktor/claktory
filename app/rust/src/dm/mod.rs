//! End-to-end direct messages on this device (dms-e2ee.md §5.3, E4): the
//! sealed store and history transfer between the person's devices (E8);
//! what Dart calls is api/dm.rs and api/dm_transfer.rs.

pub mod store;
pub mod transfer;

#[cfg(test)]
mod hostile;
#[cfg(test)]
mod rotation;

pub mod model;
#[cfg(test)]
mod native_tests;
pub mod relay;
#[cfg(test)]
mod relay_tests;
pub mod service;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ticket_tests;
pub mod tickets;

mod desktop;
pub mod migration;

pub mod api;

mod policy;
mod relay_error;
mod relay_http;
mod relay_ws;
#[cfg(test)]
mod security_tests;

#[cfg(test)]
mod api_tests;
mod validation;

#[cfg(test)]
mod dispatch_tests;

pub mod dispatch;
pub(crate) mod dispatch_migration;

#[cfg(test)]
mod inbox_tests;

// Intake remains private until authenticated transport and local confirmation are wired.
#[allow(dead_code)]
mod inbox;
pub(crate) mod inbox_migration;

#[cfg(test)]
mod trust_tests;

// Trust management remains private until a local authorization flow exists.
#[allow(dead_code)]
mod trust;
pub(crate) mod trust_migration;

pub mod dispatch_wire;
mod dispatch_wire_v2;
#[cfg(test)]
mod dispatch_wire_v2_tests;
#[cfg(test)]
mod dispatch_wire_tests;

#[allow(dead_code)]
mod binding;
pub(crate) mod binding_migration;
#[cfg(test)]
mod binding_tests;

pub mod delivery;
mod delivery_v2;
#[cfg(test)]
mod delivery_tests;
mod dispatch_result;
pub(crate) mod link_migration;
pub mod local;

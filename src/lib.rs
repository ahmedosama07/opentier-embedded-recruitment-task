pub mod server;
pub mod client;

pub mod message {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}

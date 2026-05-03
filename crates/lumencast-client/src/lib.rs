//! LSDP/1 protocol client.
//!
//! See [`Client`] for the entry point.
//!
//! ```no_run
//! use futures_util::StreamExt;
//! use lumencast_client::{Client, Event};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::builder()
//!     .url("ws://127.0.0.1:4000/ws")
//!     .token("op-token")
//!     .build()
//!     .await?;
//!
//! let mut events = client.events();
//! while let Some(ev) = events.next().await {
//!     match ev {
//!         Event::Snapshot(s) => println!("snapshot of {}", s.scene_id),
//!         Event::Delta(d)    => println!("{} patches", d.patches.len()),
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod backoff;
mod client;
mod connection;
mod events;
mod manager;

pub use client::{Client, ClientBuilder, ClientError};
pub use events::{Event, EventStream, Status};
pub use lumencast_protocol::frames::{Delta, ErrorFrame, SceneChanged, Snapshot};

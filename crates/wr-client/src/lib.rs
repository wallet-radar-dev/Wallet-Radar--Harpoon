//! Клієнт зовнішнього сервісу Wallet Radar: REST + WebSocket.
//!
//! Це **єдиний** крейт, який бачить сирі відповіді джерела. Назовні він віддає
//! лише типи без адрес — див. [`types`].

pub mod client;
pub mod parse;
pub mod types;
pub mod ws;

pub use client::{now_ms, WrClient, WrError};
pub use types::{
    Archetype, AssetLabel, CapitalGroup, ConcentrationBand, CrossAssetRow, FamilySignal, FlowKind,
    LiqSnapshot, LiqWindow, Network, Phase, RawEvent, ServerMsg, Snapshot, WhaleAsset,
};

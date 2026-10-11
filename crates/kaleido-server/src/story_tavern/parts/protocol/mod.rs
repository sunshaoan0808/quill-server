//! parts/protocol — [2026-10-06 二期①] 叙事文本协议层：散装标记（markers）+ 【回执】（receipt）。
#![allow(unused_imports)]
// [R3 2026-10-06] 协议层 lint 门禁：生产路径禁裸 unwrap（测试构建豁免）。
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod markers;
pub use markers::*;
mod receipt;
pub use receipt::*;

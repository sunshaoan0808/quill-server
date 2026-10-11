//! parts/turn — [2026-10-05 P1] 回合管线叶子模块（设计文档 §3.1 P1-P4）。
//! P1 leak_strip（思维泄漏剥离族）；P2-P4 将迁入 session_repair / bg_llm / prompt_ctx。
#![allow(unused_imports)]
// [R3 2026-10-06] 编排内核 lint 门禁：生产路径禁裸 unwrap（测试构建豁免）。
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod ctx;
pub use ctx::*;
mod session_repair;
pub use session_repair::*;
mod bg_llm;
pub use bg_llm::*;
mod prompt_ctx;
pub use prompt_ctx::*;
mod leak_strip;
pub use leak_strip::*;

mod post_parse;
pub use post_parse::*;
mod stream;
pub use stream::*;
mod checks;
pub use checks::*;
mod clock;
pub use clock::*;
mod pre_refine;
pub use pre_refine::*;
mod persist;
pub use persist::*;
mod memory;
pub use memory::*;
mod entry;
pub use entry::*;
mod director_exec;
pub use director_exec::*;

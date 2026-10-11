//! SillyTavern-compatible World Info / Lorebook scanner.
//!
//! Ported from SillyTavern `public/scripts/world-info.js` (`checkWorldInfo`,
//! `WorldInfoBuffer.matchKeys`, `convertCharacterBook`, inclusion groups,
//! probability, budget, recursion).
//!
//! Scope vs full ST:
//! - ✅ constant / keys / secondary + selectiveLogic / probability / order
//! - ✅ caseSensitive / matchWholeWords / scanDepth / inclusion groups
//! - ✅ position before|after (0|1); atDepth folded into after with marker
//! - ✅ budget (% of maxContext, char≈token*4 heuristic) + ignoreBudget
//! - ✅ one-level recursion when `recursive` setting on
//! - ✅ sticky/cooldown/delay timed effects (per-chat TimedWorldInfo store)
//! - ✅ AN top/bottom + atDepth buckets in scan result (injected around card)
//! - ✅ @@activate / @@dont_activate content decorators
//! - ✅ EM top/bottom + named outlet buckets
//! - ✅ characterFilter / generation triggers / vectorized skip
//! - ✅ {{user}}/{{char}} macros; multi-slot EM/depth/outlet in WiPromptSlots
//! - ✅ automationId collected on activation (returned to client)
//! - ✅ EM <START> blocks → user/assistant example pairs
//! - ⚠️ token budget: improved CJK/word heuristic (no external tokenizer crate)

#![allow(unused_imports)]

// [R6③ 2026-10-06] 拆分自单文件 2379 行：types/parse/serde_convert/scan/timed。
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[allow(unused_imports)]
pub mod types;
#[allow(unused_imports)]
pub use types::*;
#[allow(unused_imports)]
pub mod parse;
#[allow(unused_imports)]
pub use parse::*;
#[allow(unused_imports)]
pub mod serde_convert;
#[allow(unused_imports)]
pub use serde_convert::*;
#[allow(unused_imports)]
pub mod scan;
#[allow(unused_imports)]
pub use scan::*;
#[allow(unused_imports)]
pub mod timed;
#[allow(unused_imports)]
pub use timed::*;
#[cfg(test)]
mod tests;

//! p009 测试集（R6 自 p009.rs 单文件拆分，机械搬移零改动）。

#[allow(unused_imports)]
pub(crate) mod markers;
#[allow(unused_imports)]
pub(crate) mod guards;
#[allow(unused_imports)]
pub(crate) mod protocol_split;
#[allow(unused_imports)]
pub(crate) mod quality;
#[allow(unused_imports)]
pub(crate) use quality::*;
#[allow(unused_imports)]
pub(crate) mod common;
#[allow(unused_imports)]
pub(crate) use common::*;
#[allow(unused_imports)]
pub(crate) mod ledger;
#[allow(unused_imports)]
pub(crate) mod prompt_world;
#[allow(unused_imports)]
pub(crate) mod leak_strip;
#[allow(unused_imports)]
pub(crate) mod u11;
#[allow(unused_imports)]
pub(crate) mod domain_edges;

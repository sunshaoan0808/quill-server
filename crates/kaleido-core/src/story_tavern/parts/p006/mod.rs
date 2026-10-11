//! p006 —— dual agent 行为测试集（R6 自单文件 p006.rs 拆分，纯 #[cfg(test)]，零生产代码）。
#[cfg(test)]
mod focus;
#[cfg(test)]
mod worldline;
#[cfg(test)]
mod actor_state;
#[cfg(test)]
mod rule_check;
#[cfg(test)]
mod director_plan;

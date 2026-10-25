//! 稳定序号发号器 —— 只增不减、永不复用、删了留洞。
//!
//! 参考 dianziji.lol 实测的官方注释：
//! > 「删掉 #17 就留个洞，后面的不往前挪，**否则作者写进 instruction 的编号会集体错位**」
//!
//! # 为什么 UUID 不够
//!
//! 现有世界书/角色卡/条目都用 `Uuid::new_v4()` 发号（如 `wb-{uuid}`、`e-{uuid}`）。
//! UUID 确实**永不复用**，但它是 36 字符随机串，作者**无法手写进 instruction**。
//! dianziji 的价值不在「不复用」，而在于发一个**短、可见、可手写**的号（`lb-1`、`g3`、`#47`），
//! 让作者能在卡片的 instruction 里写「当 #17 被触发时…」，且删掉中间某条时
//! 后面的编号**不会集体前移**。
//!
//! # 三条硬规则（违反即 bug）
//!
//! 1. **只增不减** —— `next()` 单调递增。
//! 2. **永不复用** —— 删除一个已发出的序号后，下一次 `next()` 不得回填该洞。
//!    这条必须靠**持久化游标**（high-water mark）保证；靠 `max(现存)+1` 重算
//!    会在「删掉最后一条再新增」时立刻复用，恰好违反规则。
//! 3. **删了留洞** —— 同 2，后面的编号不往前挪。
//!
//! # 用法
//!
//! ```ignore
//! let mut seq = StableSeq::new();
//! assert_eq!(seq.next(), 1);
//! assert_eq!(seq.next(), 2);
//! // 2 号被作者删掉了，游标仍在 2
//! assert_eq!(seq.next(), 3); // 留洞，不复用 2
//! assert_eq!(seq.peek(), 3);
//! ```
//!
//! 游标通过 [`StableSeq::high_water`] / [`StableSeq::from_high_water`] 持久化，
//! 重启后从游标继续，不因「现存条目都删光」而归零。

use serde::{Deserialize, Serialize};

/// 单调递增发号器。`high_water` 是已发出的**最大**序号（1-based，0 = 尚未发号）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StableSeq {
    /// 已发出的最大序号。`next()` 返回 `high_water + 1`。
    #[serde(default)]
    pub high_water: u64,
}

impl StableSeq {
    /// 新建发号器，从 1 开始。
    pub fn new() -> Self {
        Self { high_water: 0 }
    }

    /// 从持久化游标恢复。用于重启后继续发号。
    pub fn from_high_water(high_water: u64) -> Self {
        Self { high_water }
    }

    /// 发下一个号。**只增不减、永不复用。**
    pub fn next(&mut self) -> u64 {
        // saturating_add：u64::MAX 时停在 MAX，绝不溢出回绕到 0（那会造成复用）。
        self.high_water = self.high_water.saturating_add(1);
        self.high_water
    }

    /// 已发出的最大序号（0 = 尚未发号）。不发号，可安全查询。
    pub fn peek(&self) -> u64 {
        self.high_water
    }

    /// 是否尚未发过号。
    pub fn is_empty(&self) -> bool {
        self.high_water == 0
    }

    /// 保留游标中的洞，但吸收「外部导入的更大编号」——
    /// 导入老数据（可能自带 `seq: 47`）后，新号必须从 48 起，不能撞上已存在的号。
    ///
    /// 只会前进，不会后退（不破坏「只增不减」）。
    pub fn observe(&mut self, seq: u64) {
        if seq > self.high_water {
            self.high_water = seq;
        }
    }

    /// 发号并格式化成带前缀的短号，如 `lb-17`、`g3`、`#47`。
    ///
    /// `prefix` 传空串则不带分隔符（配图场景用 `#` 或空）。
    pub fn next_tag(&mut self, prefix: &str) -> String {
        let n = self.next();
        if prefix.is_empty() {
            format!("#{n}")
        } else if prefix.ends_with('#') {
            // 配图场景：prefix 就是 "#"，避免拼成 "##47"。
            format!("{prefix}{n}")
        } else {
            format!("{prefix}-{n}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_one_and_increments() {
        let mut s = StableSeq::new();
        assert_eq!(s.next(), 1);
        assert_eq!(s.next(), 2);
        assert_eq!(s.next(), 3);
        assert_eq!(s.peek(), 3);
    }

    #[test]
    fn deleting_an_issued_seq_leaves_a_hole_never_refilled() {
        // dianziji 核心规则：删掉 #17 留个洞，后面的不往前挪。
        let mut s = StableSeq::new();
        for _ in 0..17 {
            s.next();
        }
        assert_eq!(s.peek(), 17);
        // 作者删掉 17 号条目 —— 平台侧不重算游标
        assert_eq!(s.next(), 18, "17 应留洞，新号是 18");
        assert_eq!(s.next(), 19);
    }

    #[test]
    fn restart_from_persisted_high_water_never_repeats() {
        let mut s = StableSeq::new();
        s.next();
        s.next();
        let persisted = s.high_water;
        // 重启：从游标恢复
        let mut s2 = StableSeq::from_high_water(persisted);
        assert_eq!(s2.next(), 3, "重启后不得从 1 重新发");
    }

    #[test]
    fn all_entries_deleted_then_new_does_not_restart_at_one() {
        // 危险场景：只按「现存条目」重算游标时，删光就会归零 → 编号被复用 → 全体错位。
        let mut s = StableSeq::new();
        s.next();
        s.next();
        s.next();
        // 现存条目被删光，但游标仍在
        assert_eq!(s.next(), 4, "删光后不得回到 1");
    }

    #[test]
    fn observe_absorbs_imported_larger_numbers_without_going_backwards() {
        let mut s = StableSeq::new();
        s.next(); // 1
        s.observe(47); // 导入老数据自带 seq=47
        assert_eq!(s.peek(), 47);
        assert_eq!(s.next(), 48, "新号必须避开已存在的 47");
        // 小值不回退
        s.observe(3);
        assert_eq!(s.peek(), 48);
        assert_eq!(s.next(), 49);
    }

    #[test]
    fn observe_zero_or_ignored_is_noop() {
        let mut s = StableSeq::new();
        s.next();
        s.observe(0);
        assert_eq!(s.peek(), 1);
        assert_eq!(s.next(), 2);
    }

    #[test]
    fn next_tag_formats_prefix_and_number() {
        let mut s = StableSeq::new();
        assert_eq!(s.next_tag("lb"), "lb-1");
        assert_eq!(s.next_tag("lb"), "lb-2");
        assert_eq!(s.next_tag("g"), "g-3");
        // 配图：prefix 自带 #，不拼成 ##47
        assert_eq!(s.next_tag("#"), "#4");
        // 空 prefix 用 #
        let mut s2 = StableSeq::new();
        assert_eq!(s2.next_tag(""), "#1");
    }

    #[test]
    fn empty_reports_unissued() {
        let s = StableSeq::new();
        assert!(s.is_empty());
        assert_eq!(s.peek(), 0);
        let mut s2 = StableSeq::new();
        s2.next();
        assert!(!s2.is_empty());
    }

    #[test]
    fn saturates_at_max_never_wraps_to_reuse() {
        let mut s = StableSeq::from_high_water(u64::MAX);
        // 溢出回绕到 0 会造成编号复用 —— 必须停在 MAX。
        assert_eq!(s.next(), u64::MAX);
        assert_eq!(s.next(), u64::MAX, "不得回绕");
    }

    #[test]
    fn serde_roundtrip_preserves_cursor() {
        let mut s = StableSeq::new();
        for _ in 0..5 {
            s.next();
        }
        let json = serde_json::to_string(&s).unwrap();
        let back: StableSeq = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.peek(), 5);
        // 旧数据缺该字段（0）也能反序列化，不炸
        let old: StableSeq = serde_json::from_str("{}").unwrap();
        assert_eq!(old.peek(), 0);
    }
}

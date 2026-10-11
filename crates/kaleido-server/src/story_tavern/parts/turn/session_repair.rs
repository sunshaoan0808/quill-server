//! parts/turn/session_repair — [2026-10-05 P2-P4] 按设计文档 §3.1 从 p006/p009 逐字节迁出。
//! sanitize_session_messages（get_session 每次读档的存量消息修复）+ clear_session_active_run。
//! 以 `use super::super::*;` 直连 parts 作用域；本文件不自带 use 头，
//! cargo fix 无桥可拆（工程纪律见 docs/REFACTOR_20260927.md）。
#![allow(unused_imports)]

use super::super::*;

pub(crate) fn sanitize_session_messages(sess: &mut kaleido_core::TavernSession) -> bool {
    let mut dirty = false;
    for msg in sess.messages.iter_mut() {
        if msg.role == "user" {
            continue;
        }
        let (clean, opts) = split_options_from_narrative(&msg.content);
        if clean != msg.content {
            msg.content = clean;
            dirty = true;
        }
        // 剥离旧消息中的【面板】标记（面板已随回合回写到 session.panels）
        let (clean_p, _panels) = split_panels_from_narrative(&msg.content);
        if clean_p != msg.content {
            msg.content = clean_p;
            dirty = true;
        }
        // 剥离旧消息中的【程序】标记（程序卡已存到 msg.program）
        let (clean_pr, _prog) = split_program_from_narrative(&msg.content);
        if clean_pr != msg.content {
            msg.content = clean_pr;
            dirty = true;
        }
        // ST-27: 剥离旧消息中的【检定】块（检定结果只以可读文本追加，块不留存）
        let (clean_ch, _checks) = split_check_from_narrative(&msg.content);
        if clean_ch != msg.content {
            msg.content = clean_ch;
            dirty = true;
        }
        if msg.options.is_empty() && !opts.is_empty() {
            msg.options = opts;
            dirty = true;
        } else if !opts.is_empty() && msg.options.is_empty() {
            msg.options = opts;
            dirty = true;
        }
        // if content still has marker somehow
        if msg.content.contains("【选项】") {
            let (c2, o2) = split_options_from_narrative(&msg.content);
            msg.content = c2;
            if msg.options.is_empty() && !o2.is_empty() {
                msg.options = o2;
            }
            dirty = true;
        }
    }
    dirty
}


pub(crate) fn clear_session_active_run(store: &TavernSessionStore, session_id: &str, run_id: Option<&str>) {
    // F2: Use atomic release_turn to prevent race with acquire_turn.
    store.release_turn(session_id, run_id);
}


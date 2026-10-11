//! crawler/parts/p002/pua —— PUA 配置（OnceLock）+ 正文解密。（R6-3① 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct PuaConfig {
    code: [[u32; 2]; 2],
    charset: [Vec<char>; 2],
}

/// Built-in default charset (last resort if config file missing).
pub(crate) fn builtin_pua_config() -> PuaConfig {
    PuaConfig {
        code: [[58344, 58715], [58345, 58716]],
        charset: [
            "D在主特家军然表场4要只v和?6别还g现儿岁??此象月3出战工相o男直失世F都平文什VO将真T那当?会立些u是十张学气大爱两命全后东性通被1它乐接而感车山公了常以何可话先pi叫轻M士w着变尔快l个说少色里安花远7难师放t报认面道S?克地度I好机U民写把万同水新没书电吃像斯5为y白几日教看但第加候作上拉住有法r事应位利你声身国问马女他Y比父xAHNsX边美对所金活回意到z从j知又内因点Q三定8Rb正或夫向德听更?得告并本q过记L让打f人就者去原满体做经K走如孩cG给使物?最笑部?员等受k行一条果动光门头见往自成处于名其发总母的死手入路进心来h时力多开已许d至由很界n小与Z想代么分生口再妈望次西风種带J?实情才这?E我神格长觉间年眼无不亲关结0友信下却重己老2音字m呢明之前高PB目太e9起稜她也W用方子英每理便四数期中C外样a海们任"
                .chars().collect(),
            "s?作口在他能并B士4U克才正们字声高全尔活者动其主报多望放hw次年?中3特于十入要男同G面分方K什再教本己结1等世N?说gu期Z外美M行给9文将两许张友0英应向像此白安少何打气常定间花见孩它直风数使道第水已女山解dP的通关性叫儿L妈问回神来S 四望前国些OvlA心平自无军光代是好却c得种就意先立z子过Yj表 么所接了名金受J满眼没部那m每车度可R斯经现门明V如走命y6E战很上f月西7长夫想话变海机x到W一成生信笑b父开内东马日小而后带以三几为认X死员目位之学远人音呢我q乐象重对个被别F也书稜D写还因家发时i或住德当ol比觉然吃去公a老亲情体太b万C电理?失力更拉物着原s工实色感记看出相路大你候2和?与p样新只便最不进Tr做格母总爱身师轻知往加从?天eH?听场由快边让把任8条头事至起点真手这难都界用法n处下又Q告地5kt岁有会果利民"
                .chars().collect(),
        ],
    }
}

/// Global PUA config: loaded once from data/fanqie_charset.json, cached forever.
pub(crate) static PUA_CONFIG: std::sync::OnceLock<PuaConfig> = std::sync::OnceLock::new();

pub(crate) fn get_pua_config() -> &'static PuaConfig {
    PUA_CONFIG.get_or_init(|| {
        let path = std::path::Path::new("data/fanqie_charset.json");
        match std::fs::read_to_string(path) {
            Ok(json_str) => {
                match serde_json::from_str::<Value>(&json_str) {
                    Ok(data) => {
                        let code = [
                            [
                                data["code_ranges"][0][0].as_u64().unwrap_or(58344) as u32,
                                data["code_ranges"][0][1].as_u64().unwrap_or(58715) as u32,
                            ],
                            [
                                data["code_ranges"][1][0].as_u64().unwrap_or(58345) as u32,
                                data["code_ranges"][1][1].as_u64().unwrap_or(58716) as u32,
                            ],
                        ];
                        let mode0: Vec<char> = data["mode0"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str().and_then(|s| s.chars().next())).collect())
                            .unwrap_or_default();
                        let mode1: Vec<char> = data["mode1"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str().and_then(|s| s.chars().next())).collect())
                            .unwrap_or_default();
                        if mode0.is_empty() || mode1.is_empty() {
                            tracing::warn!("[crawler] fanqie_charset.json has empty charset, using builtin");
                            return builtin_pua_config();
                        }
                        tracing::info!("[crawler] loaded fanqie_charset.json: mode0={} mode1={}", mode0.len(), mode1.len());
                        PuaConfig { code, charset: [mode0, mode1] }
                    }
                    Err(e) => {
                        tracing::warn!("[crawler] fanqie_charset.json parse error: {e}, using builtin");
                        builtin_pua_config()
                    }
                }
            }
            Err(_) => {
                tracing::info!("[crawler] fanqie_charset.json not found, using builtin charset");
                builtin_pua_config()
            }
        }
    })
}

/// Fanqie PUA decrypt: maps PUA codepoints to real chars using config charset.
pub(crate) fn decrypt_text(text: &str) -> String {
    let cfg = get_pua_config();
    let code = cfg.code;
    let charset = &cfg.charset;

    let has_pua = text.chars().any(|c| {
        let u = c as u32;
        (58344..=58716).contains(&u)
    });
    if !has_pua {
        return text.to_string();
    }

    let mut mode_results = Vec::new();
    for mode in 0..2 {
        let mut q_count = 0;
        let mut decoded_chars = String::new();
        for char in text.chars() {
            let uni = char as u32;
            if uni >= code[mode][0] && uni <= code[mode][1] {
                let bias = (uni - code[mode][0]) as usize;
                if bias < charset[mode].len() {
                    let mapped_char = charset[mode][bias];
                    if mapped_char == '?' {
                        q_count += 1;
                        decoded_chars.push(char);
                    } else {
                        decoded_chars.push(mapped_char);
                    }
                } else {
                    q_count += 1;
                    decoded_chars.push(char);
                }
            } else {
                decoded_chars.push(char);
            }
        }
        mode_results.push((q_count, decoded_chars));
    }

    let best_mode = if mode_results[0].0 <= mode_results[1].0 {
        0
    } else {
        1
    };
    mode_results[best_mode].1.clone()
}


// ---- Bookshelf ----


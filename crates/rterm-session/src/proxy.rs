//! proxy — SSH 跳板链构建 / 环路检测 / 深度限制
//!
//! 纯函数模块，不依赖 UI 与网络，便于单测。
//!
//! 数据模型中 `Session.proxy_jump` 嵌入的是跳板会话的一份快照
//! （由编辑对话框从已存会话复制），因此环路检测以 `Session.id` 为准：
//! 复制会保留 id，重复出现即视为环路。

use anyhow::{bail, Result};
use uuid::Uuid;

use crate::model::Session;

/// 允许的最大跳板数（不含目标会话）。
pub const MAX_JUMPS: usize = 5;

/// 构建连接顺序：最外层跳板在前，目标会话最后。
///
/// 返回的每一跳都是独立会话快照，供连接层逐跳握手使用。
pub fn build_chain(target: &Session) -> Result<Vec<Session>> {
    let mut chain = vec![target.clone()];
    let mut seen = vec![target.id];
    let mut current = target;
    let mut jumps = 0usize;

    while let Some(proxy) = current.proxy_jump.as_deref() {
        if seen.contains(&proxy.id) {
            bail!("检测到跳板环路：会话「{}」重复出现", proxy.display_label());
        }
        jumps += 1;
        if jumps > MAX_JUMPS {
            bail!("跳板层级超过 {MAX_JUMPS} 跳，请精简代理链");
        }
        seen.push(proxy.id);
        chain.push(proxy.clone());
        current = proxy;
    }

    chain.reverse();
    Ok(chain)
}

/// 跳板名称列表（最外层在前），用于对话框的链式预览。
pub fn chain_labels(target: &Session) -> Vec<String> {
    build_chain(target)
        .map(|chain| chain.iter().map(|s| s.display_label()).collect())
        .unwrap_or_default()
}

/// 选择 `candidate` 作为 `target_id` 的跳板是否会形成环路（含自选）。
pub fn selection_forms_cycle(target_id: Uuid, candidate: &Session) -> bool {
    if candidate.id == target_id {
        return true;
    }
    match build_chain(candidate) {
        Ok(chain) => chain.iter().any(|hop| hop.id == target_id),
        // 候选自身已有环路/超深：一律视为不可选。
        Err(_) => true,
    }
}

/// 校验「把 `candidate` 设为 `target_id` 会话的跳板」是否合法。
pub fn validate_selection(target_id: Uuid, candidate: &Session) -> Result<()> {
    if candidate.id == target_id {
        bail!("不能选择自身作为跳板");
    }
    let chain = build_chain(candidate)?;
    if chain.iter().any(|hop| hop.id == target_id) {
        bail!("选择该跳板会形成环路");
    }
    Ok(())
}

/// UI 过滤：`candidate` 能否作为 `target_id` 的跳板（非自身 / 无环路 / 不超深度）。
pub fn can_be_proxy_of(target_id: Uuid, candidate: &Session) -> bool {
    if candidate.id == target_id {
        return false;
    }
    match build_chain(candidate) {
        Ok(chain) => {
            let jumps = chain.len().saturating_sub(1);
            !chain.iter().any(|hop| hop.id == target_id) && jumps < MAX_JUMPS
        }
        Err(_) => false,
    }
}

/// 会话的代理链展示文案：`直连` 或 `A → B → 目标`。
pub fn describe_chain(target: &Session) -> String {
    let chain = match build_chain(target) {
        Ok(chain) => chain,
        Err(error) => return format!("代理链无效: {error}"),
    };
    chain
        .iter()
        .map(|hop| hop.display_label())
        .collect::<Vec<_>>()
        .join(" → ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn session(name: &str) -> Session {
        Session {
            id: Uuid::new_v4(),
            name: name.to_string(),
            host: format!("{name}.example.com"),
            username: "ops".into(),
            ..Session::default()
        }
    }

    #[test]
    fn direct_chain_contains_only_target() {
        let target = session("target");
        let chain = build_chain(&target).expect("直连应通过");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].id, target.id);
    }

    #[test]
    fn chain_orders_from_outermost_to_target() {
        let outer = session("a");
        let mut middle = session("b");
        middle.proxy_jump = Some(Box::new(outer));
        let mut inner = session("c");
        inner.proxy_jump = Some(Box::new(middle));
        let mut target = session("target");
        target.proxy_jump = Some(Box::new(inner));
        // 目标 → c → b → a：连接顺序必须是 a → b → c → 目标
        let chain = build_chain(&target).expect("三层跳板应通过");
        let names: Vec<&str> = chain.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "target"]);
    }

    #[test]
    fn detects_cycle_by_repeated_id() {
        let target = session("target");
        let mut proxy = session("proxy");
        proxy.proxy_jump = Some(Box::new(target.clone()));
        let mut cyclic = target.clone();
        cyclic.proxy_jump = Some(Box::new(proxy));
        let error = build_chain(&cyclic).expect_err("环路必须报错");
        assert!(format!("{error}").contains("环路"));
    }

    #[test]
    fn rejects_more_than_max_jumps() {
        // 构造 6 跳：a1 <- a2 <- ... <- a6 <- target
        let mut previous = session("hop1");
        for index in 2..=6 {
            let mut next = session(&format!("hop{index}"));
            next.proxy_jump = Some(Box::new(previous));
            previous = next;
        }
        let mut target = session("target");
        target.proxy_jump = Some(Box::new(previous));
        let error = build_chain(&target).expect_err("超过 5 跳必须报错");
        assert!(format!("{error}").contains("5"));
    }

    #[test]
    fn allows_exactly_max_jumps() {
        let mut previous = session("hop1");
        for index in 2..=5 {
            let mut next = session(&format!("hop{index}"));
            next.proxy_jump = Some(Box::new(previous));
            previous = next;
        }
        let mut target = session("target");
        target.proxy_jump = Some(Box::new(previous));
        let chain = build_chain(&target).expect("5 跳应通过");
        assert_eq!(chain.len(), 6);
    }

    #[test]
    fn selection_rejects_self_and_cycle() {
        let target = session("target");
        let mut candidate = session("candidate");
        candidate.proxy_jump = Some(Box::new(target.clone()));
        assert!(selection_forms_cycle(target.id, &target));
        assert!(selection_forms_cycle(target.id, &candidate));
        assert!(!selection_forms_cycle(target.id, &session("other")));
        assert!(validate_selection(target.id, &candidate).is_err());
    }

    #[test]
    fn can_be_proxy_filters_self_cycle_and_depth() {
        let target = session("target");
        assert!(!can_be_proxy_of(target.id, &target), "自身不可选");
        assert!(can_be_proxy_of(target.id, &session("other")));

        // 候选已有环路 → 不可选
        let mut cyclic = session("cyclic");
        cyclic.proxy_jump = Some(Box::new(cyclic.clone()));
        assert!(!can_be_proxy_of(target.id, &cyclic));

        // 候选已有 5 跳 → 再加目标会超限
        let mut previous = session("hop1");
        for index in 2..=6 {
            let mut next = session(&format!("hop{index}"));
            next.proxy_jump = Some(Box::new(previous));
            previous = next;
        }
        assert!(!can_be_proxy_of(target.id, &previous));
    }

    #[test]
    fn describe_chain_shows_arrow_path() {
        let outer = session("a");
        let mut target = session("target");
        target.proxy_jump = Some(Box::new(outer));
        assert!(describe_chain(&target).contains("→"));
    }
}

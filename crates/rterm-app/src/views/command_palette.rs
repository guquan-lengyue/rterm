//! 命令面板（Ctrl+P）— 纯状态与模糊过滤
//!
//! UI 由 MainWindow 渲染；本模块只负责「可被搜索的条目列表 + 查询状态」，
//! 便于在没有 gpui 的环境里做单元测试。

use uuid::Uuid;

/// 命令面板执行的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    OpenTerminal(Uuid),
    OpenSftp(Uuid),
    OpenMonitor(Uuid),
    NewSession,
    OpenSettings,
    ImportSessions,
    ExportSessions,
    ExportDiagnostics,
}

/// 一条可搜索命令。
#[derive(Clone, Debug)]
pub struct PaletteItem {
    pub label: String,
    pub hint: String,
    pub action: PaletteAction,
}

impl PaletteItem {
    pub fn new(label: impl Into<String>, hint: impl Into<String>, action: PaletteAction) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            action,
        }
    }
}

/// 命令面板状态。
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    items: Vec<PaletteItem>,
}

impl PaletteState {
    pub fn new(items: Vec<PaletteItem>) -> Self {
        Self {
            query: String::new(),
            selected: 0,
            items,
        }
    }

    /// 当前查询命中的条目（按匹配度降序；查询为空时保持原顺序）。
    pub fn visible(&self) -> Vec<&PaletteItem> {
        if self.query.trim().is_empty() {
            return self.items.iter().collect();
        }
        let mut scored: Vec<(i32, &PaletteItem)> = self
            .items
            .iter()
            .filter_map(|item| fuzzy_score(&self.query, &item.label).map(|score| (score, item)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, item)| item).collect()
    }

    /// 同步查询串（TextInput 内容变化时调用），并复位选择。
    pub fn set_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.selected = 0;
        }
    }

    /// 追加一个字符（输入框内容变化后调用）。
    pub fn type_char(&mut self, ch: char) {
        self.query.push(ch);
        self.selected = 0;
    }

    /// 删除最后一个字符。
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// 在当前可见条目中移动选择（越界自动夹取）。
    pub fn move_selection(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let max = count - 1;
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, max as isize) as usize;
    }

    /// 当前选中的条目。
    pub fn selected_item(&self) -> Option<&PaletteItem> {
        self.visible().into_iter().nth(self.selected)
    }
}

/// 模糊匹配：query 的字符需按顺序出现在 text 中（大小写不敏感）。
///
/// 评分启发式：连续命中、词首命中加分，文本越长略微减分。
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    let haystack: Vec<char> = text.to_lowercase().chars().collect();

    let mut score = 0_i32;
    let mut cursor = 0_usize;
    let mut previous: Option<usize> = None;

    for expected in needle {
        let mut found = None;
        for index in cursor..haystack.len() {
            if haystack[index] == expected {
                found = Some(index);
                break;
            }
        }
        let index = found?;
        score += 10;
        if previous.map(|prev| prev + 1 == index).unwrap_or(false) {
            score += 15;
        }
        if index == 0 || !haystack[index - 1].is_alphanumeric() {
            score += 20;
        }
        previous = Some(index);
        cursor = index + 1;
    }

    Some(score - (haystack.len() as i32) / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, hint: &str) -> PaletteItem {
        PaletteItem::new(label, hint, PaletteAction::NewSession)
    }

    #[test]
    fn fuzzy_matches_subsequence_and_rejects_gaps() {
        assert!(fuzzy_score("db", "db-prod").is_some());
        assert!(fuzzy_score("dbp", "db-prod").is_some());
        assert!(
            fuzzy_score("pb", "db-prod").is_none(),
            "顺序不对不应命中（p 之后没有 b）"
        );
        assert!(fuzzy_score("xyz", "db-prod").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_prefix_and_consecutive_matches() {
        let prefix = fuzzy_score("db", "db-prod").unwrap();
        let scattered = fuzzy_score("db", "prod-db-backup").unwrap();
        assert!(prefix > scattered, "词首命中应排在后面散点命中之前");
    }

    #[test]
    fn visible_filters_and_orders() {
        let mut state = PaletteState::new(vec![
            item("打开终端 · prod-db", "终端"),
            item("打开终端 · web-01", "终端"),
            item("新建会话…", "会话"),
        ]);

        assert_eq!(state.visible().len(), 3, "空查询显示全部");

        state.type_char('d');
        state.type_char('b');
        let labels: Vec<&str> = state.visible().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["打开终端 · prod-db"], "只保留命中项");
        assert_eq!(state.selected_item().unwrap().label, "打开终端 · prod-db");

        state.backspace();
        assert_eq!(state.query, "d");
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut state = PaletteState::new(vec![item("a", ""), item("b", "")]);
        state.move_selection(1);
        assert_eq!(state.selected, 1);
        state.move_selection(5);
        assert_eq!(state.selected, 1, "越界夹到末尾");
        state.move_selection(-5);
        assert_eq!(state.selected, 0, "越界夹到开头");

        let empty = PaletteState::new(Vec::new());
        assert!(empty.selected_item().is_none());
    }
}

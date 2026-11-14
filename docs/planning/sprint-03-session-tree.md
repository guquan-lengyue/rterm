# Sprint 3 — 会话树 + 编辑对话框 + 启动退出

> **状态**：✅ 已完成（2026-11-14）
> **前置**：S1（AppState + EventBus 已就绪）+ S2（SessionStore + Session 模型 + DPAPI 加密桥）
> **目标**：SessionTree View 真实化 + 编辑对话框 Modal + 启动加载 + 退出 save
> **注意**：本 Sprint 的 View **全部 stub 后端连接**（还没有 SSH），双击"打开终端"先 emit 事件 + log
>
> **实现偏差说明**：
> - 3.1.2：右键菜单为自绘（bezel-gpui 无原生 Menu），暂不含「复制 / 打开终端 / 打开 SFTP」与删除二次确认
> - 3.2.2：密码编辑时不回显明文，显示 8 个掩码点；未修改则保存时沿用原密码
> - 3.2.5：测试结果以内联状态文字显示（绿/红），未用独立 Modal
> - 3.4.1：导出/导入为固定路径 `%APPDATA%\rterm\sessions-export.json`（暂无文件选择器），导入策略 Skip
>
> **⚠️ EventBus 真实 API（必须对齐 S1 落地）**：
> - emit：`app.emit(&app.global::<AppState>().event_bus, AppEvent::Xxx { ... })`
> - subscribe：`cx.subscribe(&app_state.event_bus, |app, ev| { ... })`
> - AppState 拿法：`app.global::<AppState>()`
>
> **📐 UI 原型**：详见 `docs/architecture/ui-prototypes.md` §1 MainWindow + §2 SessionTree + §3 SessionEditDialog + §10 存储路径

---

### 3.1 SessionTree View 真实化

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.1.1 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | **替换 S1 占位为真实树形**：按 `session.folder.split('/')` 建树；folder 节点前 `▶`/`▼` 展开折叠；叶子节点显示 `session.name` + 主机名灰色副标题；图标区分 folder / session / 连接状态（S3 先全 gray 状态图标） | 构造 5 个 Session（2 folder）→ 树形显示正确 + 能折叠展开 |
| 3.1.2 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | **右键菜单**：新建 / 编辑 / 复制 / 删除 / 导出 / 打开终端（灰）/ 打开 SFTP（灰）—— gpui MenuItem + `window.open_menu()`；删除弹二次确认 Modal | 右键弹菜单；点删除 → confirm Modal → 真删 |
| 3.1.3 | 0.25h | `crates/rterm-app/src/views/session_tree.rs` | **双击 / Enter** → `app.emit(&state.event_bus, SessionConnectRequested { id })`；gpui 空实现先 log，S5 接 | debug log 能看到事件发出 |
| 3.1.4 | 0.25h | `crates/rterm-app/src/views/session_tree.rs` | **订阅 store 变更**：`cx.subscribe(&state.event_bus, AppEvent::SessionCreated/Updated/Deleted)` → `cx.notify()` | upsert 一个 Session → 左侧树立刻刷新 |

### 3.2 会话编辑对话框（Modal）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.2.1 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | `show_edit_dialog(app: &mut App, editing: Option<Session>)` — gpui Modal；基础字段：名称 / 主机 / 端口(default 22) / 用户名 / 分组 folder（下拉+自由输入） | Modal 弹出；空字段 placeholder；编辑模式预填充 |
| 3.2.2 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | **认证 Tab**（三选一 Radio）：Password → 密码输入框（明文显示）；PrivateKey → 文件选择 + 可选口令；Agent → 空（S5 接 ssh-agent 枚举） | 三种都能填 + 切换 Tab 不丢数据 |
| 3.2.3 | 0.25h | `crates/rterm-app/src/views/session_dialog.rs` | **高级 Tab**：跳板机可填（简化：只填 first host）+ KeepAlive + 超时 + 压缩 + 主题覆盖 + ☑ 全局默认 checkbox（勾上后相关字段灰显） | 勾默认 → 端口等灰显不可改 |
| 3.2.4 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | **保存**按钮：读表单 → 构造 Session → DPAPI 加密密码/口令 → `SessionStore.upsert(session)` → `app.emit(&state.event_bus, SessionUpdated/Created { id })` → Modal.close()；取消直接 close | 保存 → tree 刷新 → 密码字段 JSON 里是 base64 密文 |
| 3.2.5 | 0.25h | `crates/rterm-app/src/views/session_dialog.rs` | **测试连接**按钮：TCP connect(5s timeout) → 成功弹 green Modal + "TCP OK"；失败弹 red Modal + err 描述（先不做 SSH 握手，S5 接） | 对 192.168.1.95 能返回成功 |

### 3.3 启动加载 / 退出保存

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.3.1 | 0.25h | `crates/rterm-app/src/main.rs` | 启动时：`dirs::data_dir()` + `"rterm/sessions.json"` → `SessionStore::load(path, dpapi_decrypt)` + `Settings::default()`（S4 再做 Settings save）；store 文件不存在就空 store | 重启 app → 会话还在 |
| 3.3.2 | 0.25h | `crates/rterm-app/src/main.rs` | 退出钩子（`App::on_app_quit` 或 Ctrl+C handler）：`SessionStore::save(path, dpapi_encrypt)` | Ctrl+C → JSON 更新时间戳刷新 |

### 3.4 验收

| # | 粒度 | 验收 |
|---|------|------|
| 3.4.1 | 0.25h | 启动 → 空 SessionTree → 右键新建 → 填 db-prod → 保存 → 出现树节点（可展开 folder）→ 退出重启 → 还在 → 右键导出 JSON → cat JSON 看密码是 base64 密文 → 清库 → 导入恢复 |
| 3.4.2 | 0.25h | edit 已有会话 → 密码填新值 → 保存 → 二次打开编辑对话框 → 密码显示为 8 星号（dpapi 解出明文，但 UI 不回显明文） |
| 3.4.3 | 0.25h | `cargo check --workspace` 0 error；`cargo test --workspace` 之前的 20 个测试全过 |

---

### 文件清单

```
修改：
  crates/rterm-app/src/main.rs           ← 启动/退出持久化
  crates/rterm-app/src/views/session_tree.rs  ← 重写（占位→真实）

新增：
  crates/rterm-app/src/views/session_dialog.rs
  crates/rterm-app/src/views/mod.rs（如有，导出新模块）
```

---

## Sprint 3 总结（2026-11-14）

### 交付内容

| 模块 | 实现 |
|------|------|
| SessionTree | folder 分组 + ▼/▶ 展开折叠 + 选中高亮 + 右键菜单（新建/编辑/删除/导出/导入）+ 双击发起连接 |
| SessionEditDialog | 三 Tab（基础/认证/高级）+ 密码 8 位掩码 + 测试连接（TCP 5s）+ 保存/取消 |
| TextInput | 自绘 + IME（EntityInputHandler）+ 密码模式 + Backspace/Delete/←/→/Home/End/Ctrl+A |
| 主题系统 | `theme.rs` Theme trait + CatppuccinMocha / WindowsLight，全项目无硬编码 rgb |
| 持久化 | `%APPDATA%\rterm\sessions.json`：启动 DPAPI 解密加载，退出加密保存（Arc<Mutex<SessionStore>> 共享） |
| 导入导出 | `%APPDATA%\rterm\sessions-export.json`：DPAPI 密文导出 / 导入（ConflictPolicy::Skip） |
| 事件总线 | EventBusHost（EventEmitter<AppEvent>）+ 各 View subscribe/emit |

### 提交记录

| Commit | 说明 |
|--------|------|
| `237ee22` | SessionTree + EventBusHost + TextInput + SessionEditDialog 初版 |
| `2a5aa7a` | 全局主题系统 + 文字居中 + 去硬编码颜色 |
| `06a045b` | 退出保存持久化修复 + 右键菜单 + 双击连接 |
| `98f42e3` | TextInput 键盘编辑（Backspace/Delete/方向键/Home/End/Ctrl+A） |
| `b128354` | 测试连接 + 密码掩码 + 会话导入导出 |

### 验证结果

- `cargo check --workspace`：0 error
- 测试：10 (crypto) + 10 (session) + 2 + 8 = 30 项全过
- E2E（真实键鼠操作）：新建/编辑/删除会话、退出保存、重启加载、密码 8 位掩码且未修改不覆盖、TCP 测试连接、导出后删除再导入恢复 —— 全部通过

### 遗留项（转入后续 Sprint）

| 项 | 去向 |
|----|------|
| 双击连接只是 emit `SessionConnectRequested`，无订阅者 | Sprint 5（5.5.1） |
| 无终端视图 / PTY | Sprint 5 + Sprint 6 |
| 右键菜单缺「复制会话」「打开终端/SFTP（灰置）」 | Sprint 5/7 顺带补 |
| 删除无二次确认 | S4/S6 UI 打磨时补 |
| 导入导出固定路径，无文件选择器 / 口令 / diff 预览 | Sprint 4（4.2） |
| 高级 Tab 无跳板机、无「全局默认」勾选 | Sprint 4（设置）/ Sprint 5（跳板机） |
| 密码掩码固定 8 点（非真实长度） | 可接受，后续如需精确长度再调 |


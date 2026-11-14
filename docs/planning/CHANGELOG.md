# CHANGELOG — Sprint 完成记录

> 每个 Sprint 完成后，把总结追加到这里。格式统一：状态 / commit / 交付清单 / 代码统计 / 架构落地 / 踩坑 / 验收 checklist / 遗留。

---

## Sprint 4 — 设置 + 导入导出

**状态**：✅ COMPLETED | **Commit**：`e7a9574` | **时间**：2026-11-14
**前置**：S3（SessionTree / SessionEditDialog / 启动退出持久化）
**后续**：S5（SSH 连接 + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 4.1.1 | SettingsView：左侧 7 Tab + 右侧表单 | `views/settings.rs` | ✅ 每 Tab 内容独立；`Ctrl+,` 打开 |
| 4.1.2 | SSH 默认值 / SFTP 模式 / 监控 / 高级目录 | `views/settings.rs` | ✅ 保存后写回 `AppState.settings` 并广播事件 |
| 4.1.3 | 终端外观：主题 / 字体 / 字号 / 光标 / 滚动 + 预览 | `views/settings.rs` + `theme.rs` | ✅ 切 Dracula 预览立刻变紫；字号联动预览 |
| 4.1.4 | 保存 / 取消 / 恢复默认 | `views/settings.rs` | ✅ 取消还原主题；恢复默认重置全部字段 |
| 4.1.5 | Settings 持久化 | `state.rs` + `main.rs` | ✅ `%APPDATA%\rterm\settings.json`，重启保留 |
| 4.2.1 | 导出 Modal：Scope / DPAPI·AES-GCM / rfd 路径 | `views/import_export.rs` | ✅ 全部/文件夹/多选；DPAPI 本机可解，口令文件跨机可用 |
| 4.2.2 | 导入 Modal：口令 / 冲突策略 / diff 预览 / 报告 | `views/import_export.rs` + `export.rs` | ✅ 错口令显示解密失败；正确口令预览 + 更新 2 条 |
| 4.2.3 | 入口：设置 Tab + SessionTree 右键菜单 | `main_window.rs` | ✅ 两处入口打开同一个 Modal |
| — | 主题扩展 + 注册表 | `theme.rs` | ✅ Dracula / One Dark；`all_themes()` / `theme_by_name()` |
| — | 导入预览 API | `rterm-session/src/export.rs` | ✅ `preview_import` 复用 `parse_and_decrypt` |

### 测试 & 验收

| 项 | 结果 |
|----|------|
| `cargo check --workspace` | ✅ 0 error |
| `cargo test --workspace` | ✅ 35 passed, 0 failed |
| E2E：终端主题 Dracula → 保存 → 重启 | ✅ 重启后仍是 Dracula |
| E2E：DPAPI 导出 2 个会话 → 导入 Overwrite | ✅ 数量一致，report 更新 2 |
| E2E：AES-GCM 口令 "abc" → 错误口令预览 | ✅ 显示「导入解密失败 — 可能需要口令」 |
| E2E：`Ctrl+,` | ✅ 设置弹窗打开 |

### 踩坑 & 解法（5 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | `App::on_action` 里同步 `WindowHandle::update` 报 `window not found` | action 派发期间窗口被临时移出 `App.windows`，改用 `app.defer` 延迟到事件处理结束后再打开 |
| 2 | 根节点 `.on_action` 在无聚焦元素时不触发 | 无焦点时 action 从 dispatch tree 根节点冒泡，不进入控件树；改用 `App::on_action` 全局监听 |
| 3 | gpui 0.3 没有 `min_h` / 滚动容器 | 表单布局保持紧凑；长列表限制展示条数（选择 24 / 预览 12） |
| 4 | 导入错误链过长把 footer 按钮挤出卡片 | 只显示顶层错误（`{e}` 而非 `{e:#}`）+ 消息区 `truncate()` |
| 5 | rfd 首次引入 + 屏幕坐标随 DPI/窗口位置变化 | 锁定依赖后用像素扫描定位按钮，坐标按 1.5 DPI 换算 |

### 遗留

| 项 | 后续 |
|----|------|
| 选择会话 / 预览列表无滚动，超过上限只显示前 N 条 | S10 稳定性阶段补滚动容器 |
| 字体为 4 个预设，未枚举系统字体 | S6 终端渲染时补 |
| 删除会话二次确认仍未做（S3 遗留） | S10 |
| 日志目录 / 数据目录首页仅存储，尚未被终端与日志模块使用 | S6 / S9 |

---

## Sprint 2 — 会话数据层

**状态**：✅ COMPLETED | **Commit**：`3bbc1c16` | **时间**：2026-11-13
**前置**：S1（crypto + event_bus + app_state + 主窗口壳）
**后续**：S3（会话 UI 层）/ S4（SSH + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 2.1 | Session 模型 + AuthMethod | `model.rs` | ✅ `Session::default()` 可构造；三个 AuthMethod 变体 |
| 2.2 | SessionStore 内存 CRUD | `store.rs` | ✅ upsert 同 id 更新；delete 返回 bool |
| 2.3 | JSON 持久化 + 原子写 | `store.rs` save/load | ✅ save→rename→tmp 清理；文件不存在返回空 store |
| 2.4 | 加密桥接 DPAPI + AES-GCM | `crypto_bridge.rs` | ✅ 密码字段 version=0↔1↔2 三态转换 |
| 2.5 | 导入导出 + 冲突策略 | `export.rs` | ✅ Overwrite/Skip/Rename 三种 + ExportScope + ImportReport |
| 2.6 | OpenSSH config 解析 | `openssh.rs` | ✅ Host/HostName/Port/User/IdentityFile/ProxyJump |
| — | S1 占位替换 | `rterm-app/Cargo.toml` + `state.rs` | ✅ 删掉空 SessionStore，改用 `rterm_session::SessionStore` |
| — | workspace 依赖 | `Cargo.toml` + `rterm-session/Cargo.toml` | ✅ base64=0.22 新增 |

### 测试

| 文件 | 数量 | 全部通过 |
|------|------|----------|
| store_tests.rs | 8 | ✅ new/upsert×2/delete/file-not-found/roundtrip/atomic/encrypted |
| export_tests.rs | 10 | ✅ DPAPI roundtrip + 3 scope + 3 conflict policy + AES-GCM correct/wrong |
| openssh_tests.rs | 2 | ✅ 基本 Host 解析 + HostName 默认 Host |
| **总计** | **20** | **✅ 20/20** |

### 踩坑 & 解法（7 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | rterm-session/Cargo.toml 没加 base64 | workspace Cargo.toml 加 `base64 = "0.22"` + rterm-session 加 `base64.workspace = true` |
| 2 | store.rs SessionFile 缺 `use serde::{Serialize, Deserialize}` | 显式 import |
| 3 | openssh.rs 用了 nightly 的 `&str::leak()` | 重写 split_into_host_blocks 返回 `Vec<(String, String)>`（全所有权） |
| 4 | rterm-app/Cargo.toml 没加 rterm-session 依赖 | workspace Cargo.toml 顶层声明了不等于各 crate 自动继承；每个 crate 自己 Cargo.toml 必须显式写 `rterm-session.workspace = true` |
| 5 | Session struct 缺 `PartialEq` | 加 derive（chrono::DateTime<Utc> + Uuid 都实现了 PartialEq，OK） |
| 6 | openssh.rs doc-comment 里的 ``` 被当 doctest 编译 | 改成 ```text 标记 |
| 7 | wrong passphrase 测试不 fail — Session::default() 密码是空，加密路径被跳过 | 测试里手工构造带明文密码的 Session（version=0） |

### 验收 Checklist

- [x] `cargo check --workspace` → **0 ERROR**
- [x] `cargo test -p rterm-session --lib --tests` → **20 passed, 0 failed**
- [x] `cargo build -p rterm-app` → 无 error（只有 S1 骨架阶段的 dead_code warning）
- [x] JSON 持久化原子写：先 .tmp 再 rename，无半截文件
- [x] EncryptedString 密文 base64：JSON 里不可读密码
- [x] rterm-app/AppState.session_store 类型 → `rterm_session::SessionStore`（替换 S1 占位成功）

### 遗留

| 项 | 后续 |
|----|------|
| CoTaskMemFree 还没修 | Sprint 9 |
| crypto_bridge.rs 还没 ProxyJump 嵌套 AuthMethod 加密/解密（当前已递归） | 已做 ✅ |
| EventBus 还没 emit（upsert/delete broadcast 事件） | Sprint 3 UI 层接上 |
| Settings 持久化 | Sprint 3 |

---


## Sprint 1 — 底层基础设施

**状态**：✅ COMPLETED | **Commit**：`f49a37e` + `40f3f32` | **时间**：2026-11-13
**前置**：S0（gpui 空窗口骨架）
**后续**：S2（会话数据层）/ S3（会话 UI 层）/ S4（SSH + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 1.1 | rterm-crypto DPAPI | `crates/rterm-crypto/src/lib.rs` | ✅ CryptProtectData / CryptUnprotectData 往返 |
| 1.2 | rterm-crypto AES-GCM + PBKDF2 | 同上 | ✅ 600K iter PBKDF2-SHA256 + 16B salt + 12B nonce |
| 1.3 | crypto 单元测试 | `crates/rterm-crypto/tests/crypto_tests.rs` | ✅ **10/10 通过**（18.62s） |
| 1.4 | EventBus + AppEvent | `crates/rterm-app/src/event.rs` | ✅ 28 个事件变体覆盖会话/终端/SFTP/监控/传输/UI |
| 1.4 | AppState + Settings + Global impl | `crates/rterm-app/src/state.rs` | ✅ `App::set_global(AppState::new())` 成功 |
| 1.5 | 主窗口三栏 gpui | `views/main_window.rs` + `session_tree.rs` + `status_bar.rs` | ✅ 窗口弹出 PID 7360，MainWindowHandle 非零 |
| P0 阻塞 | Perl + vendored-openssl | `scripts/msvc-env.ps1` 补丁 | ✅ `cargo check --workspace` 完整 8 crate 通过（9m13s） |
| 额外 | 测试服务器集成 | SSH ed25519 key + authorized_keys | ✅ 密码 + 密钥双认证通，vim/tmux/htop/nano 全预装 |

### 代码统计

| crate | 新增行数 | 测试 |
|-------|---------|------|
| rterm-crypto | ~170 行 | 10 单元测试 |
| rterm-app | ~430 行 | 0（占位 View 不需要） |
| **总计** | **~600 行** | **10/10 绿** |

### 架构落地图

```
┌─ rterm-app ─────────────────────────────────────┐
│ main.rs: gpui_platform.application()             │
│   ├─ app.set_global(AppState)                    │
│   └─ open_window → MainWindow Entity             │
│        ├─ .child(SessionTreeView Entity)  240px  │
│        └─ .child(StatusBar Entity)         28px  │
│                                                  │
│ event.rs: EventBus(空 struct) + EventEmitter     │
│   └─ AppEvent 28 variants                        │
│                                                  │
│ state.rs: Settings(12 fields) + SessionStore +   │
│           TabRegistry + AppState + impl Global   │
└──────────────────────────────────────────────────┘
         │
         ▼ 依赖
┌─ rterm-crypto ───────────────────────────────────┐
│ dpapi_encrypt / dpapi_decrypt  (windows 0.58)    │
│ aesgcm_encrypt / aesgcm_decrypt (aes-gcm 0.10)  │
│ derive_key PBKDF2-SHA256 (hmac + sha2)           │
└──────────────────────────────────────────────────┘
```

### 踩坑 & 解法（5 个坑）

| # | 问题 | 根因 | 解法 |
|---|------|------|------|
| 1 | `windows::Win32::Security::Cryptography::DATA_BLOB` 不存在 | windows 0.58 改名为 `CRYPT_INTEGER_BLOB` | 直接改名 |
| 2 | `EventBus` 包装 `SubscriberSet`（private） | bezel-gpui 0.3 不让外部碰 SubscriberSet | 用 **空 struct + `impl EventEmitter<T>` marker trait**，gpui 内部自动管理 |
| 3 | Render 里不能 `.child(session_tree)`（impl IntoElement 不存在） | 只有 `Entity<V: Render>` 能嵌进 `.child()`，Render struct 本身不行 | **main.rs 里先 `app.new()` 挂 Entity**，Render 存 Entity ID 当字段，render 时 `.child(entity)` |
| 4 | `AppState` 没有实现 `gpui::Global` | `app.set_global::<G: Global>()` 有 trait bound | 空 impl `impl gpui::Global for AppState {}` 即可 |
| 5 | vendored-openssl 报 `perl not found` | Windows OpenSSL Configure 需要 Perl | Strawberry Perl 5.42.3 已装在 `C:\Strawberry\perl\bin`，加到 msvc-env.ps1 PATH |

### 验收 Checklist

- [x] `cargo test -p rterm-crypto` → 10 passed, 0 failed
- [x] `cargo build -p rterm-app` → 无 error（warning 可接受）
- [x] `cargo check --workspace` → 完整 8 crate 过（含 ssh2 vendored-openssl + alacritty_terminal）
- [x] 启动 `rterm-app.exe` → PID 7360，MainWindowHandle ≠ 0
- [x] Ctrl+C 退出干净，无 panic

### 遗留

| 项 | 状态 | 后续 |
|----|------|------|
| `CoTaskMemFree` 没调用 | 暂跳过（泄漏几百字节无害） | Sprint 9 修 |
| SessionStore / TabRegistry 空占位 | Sprint 2/5 填充 | state.rs 里 Vec\<Uuid\> 换成 Vec\<Session\> |
| EventBus 还没 `.emit()`/`.subscribe()` 调用点 | Sprint 2+ 接上 | 骨架阶段只需要 EventEmitter trait 存在 |
| MSVC + Perl PATH 自动化 | ✅ 已进 `scripts/msvc-env.ps1` | 每次 `. .\scripts\msvc-env.ps1` 生效 |

### 测试服务器信息

| 项 | 值 |
|----|-----|
| 地址 | `192.168.1.95` |
| 用户 | `ubuntu` |
| 密码 | `liang@123..` |
| 密钥 | `C:\Users\wst_k\.ssh\rterm_test`（已加 authorized_keys） |
| OS | Ubuntu 26.04.1 LTS |
| CPU | 4 核 / RAM 7.7 GB |
| 预装工具 | vim / tmux / htop / nano / less / python3 |
| 延迟 | ping 3ms |

---

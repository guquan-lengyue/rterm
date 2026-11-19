# Sprint 13 — 会话管理增强 + SSH 跳板代理 / 端口转发（v1.2）

> **状态**：✅ 编码完成 + Ubuntu 实机 smoke 通过（2026-11-19，跳板链待第二台主机）
> **前置**：[S2](./sprint-02-session-data.md) ✅（Session / SessionStore）+ [S3](./sprint-03-session-tree.md) ✅（会话树 / 编辑对话框）+
> [S4](./sprint-04-settings-io.md) ✅（导入导出）+ [S5](./sprint-05-ssh-terminal.md) ✅（SSH 连接）；
> 与 [S12](./sprint-12-terminal-sftp-dock.md) 并行时注意 `main_window.rs` 菜单/事件冲突
> **目标**：补齐会话管理体验（分组、图标、空白区菜单、顶部菜单栏、菜单职责分离）、SSH 跳板代理与
> 端口转发；会话文件升级到 schema v2，保持旧数据可读。

---

## 需求与验收

| # | 需求 | 验收 |
|---|------|------|
| R1 | SSH 服务器代理（跳板机） | 会话可指定已有会话作为跳板；终端 / SFTP / 监控均经代理连接；支持链式跳板（A→B→C）；环路与超过 5 跳时保存/连接前报错 |
| R2 | 会话导入导出完善 | 统一入口；支持全部 / 分组 / 多选导出；OpenSSH config 有 UI 入口；冲突策略、口令加密、导入预览保留；导出包含分组、图标、代理、端口转发 |
| R3 | 分组命名与下拉选择 | 可新建 / 重命名 / 删除分组，空分组重启后仍保留；会话编辑中的分组改为可搜索下拉 + 内联新建，不再自由输入造成错名 |
| R4 | 空白处右键功能列表 | 会话树空白区域右键显示：新建会话、新建分组、导入会话…、展开全部、折叠全部 |
| R5 | 会话可修改图标 | 会话编辑对话框提供预设图标选择；右键「更改图标…」可直接修改；会话树按图标渲染，未设置为默认图标 |
| R6 | 右键菜单职责分离 | 空白 / 分组 / 会话三类菜单独立渲染与独立动作枚举；全局功能（导入、设置）不再出现在单会话菜单；删除项危险色且需确认 |
| R7 | 顶部菜单栏 | 窗口顶部新增常驻菜单栏（会话 / 连接 / 视图 / 帮助）；下拉菜单与右键菜单共用组件与主题；F10 或点击打开，Esc / 点击外部关闭；无选中会话时连接类菜单项禁用 |
| R8 | SSH 端口转发 | 每个会话可维护转发规则（v1 支持本地 `-L` 与远程 `-R`，动态 `-D` 为可选延伸）；规则可启停、可自动启动；独立「端口转发」Tab 展示状态与错误；默认仅绑定 `127.0.0.1` |

---

## 现状盘点（本次要解决的缺口）

| 能力 | 现状 | 缺口 |
|------|------|------|
| 代理 | `Session.proxy_jump: Option<Box<Session>>` 已建模，OpenSSH 解析可写入 | `rterm-ssh` 完全未使用；对话框无输入；无环路校验 |
| 端口转发 | 无任何实现 | 无规则模型、无转发运行时、无 UI 入口 |
| 导入导出 | `export.rs` 已有范围/加密/冲突策略；`import_export.rs` 有完整对话框 | 入口分散；OpenSSH 导入无 UI；导出不含新字段 |
| 分组 | 仅由 `session.folder`（`/` 分隔）派生，无独立结构 | 无法新建空分组、重命名、删除；编辑框为自由文本 |
| 右键菜单 | 单会话菜单混入新建/导入/设置等全局项；文件夹菜单仅 4 项；空白处无菜单 | 职责不清、缺少空白区入口 |
| 菜单栏 | 顶部只有 Tab 栏与「设置」按钮，功能入口依赖快捷键/右键 | 无传统菜单栏，新用户难以发现功能；无统一动作入口 |
| 图标 | 会话固定 🖥️ | 无 `icon` 字段与选择 UI |

---

## 架构设计（分层）

```
rterm-session
├── model.rs        Session 增加 icon；SessionFile 增加 folders，FILE_VERSION=2
├── store.rs        分组 CRUD（新建/重命名/删除空分组）+ 迁移
├── proxy.rs（新）  跳板链构建 / 环路检测 / 深度上限（纯函数，可单测）
└── export.rs       导出/导入 schema v2；兼容 v1

rterm-ssh
└── lib.rs          connect() 支持代理链：channel_direct_tcpip 逐跳转发，持有中间 Session

rterm-forward（新）
└── lib.rs          转发规则校验 + 独立 SSH 连接的转发 worker（Local / Remote / 可选 Dynamic）

rterm-app
├── views/context_menu.rs（新）  三类菜单状态 + 统一菜单项/分隔线渲染
├── views/menu_bar.rs（新）      顶部菜单栏 + 下拉菜单 + MenuCommand 事件
├── views/port_forward.rs（新）  端口转发 Tab：规则列表 / 状态 / 启停 / 增删改
├── views/session_tree.rs        空白区右键、分组行右键、显式空分组、自定义图标
├── views/session_dialog.rs      分组下拉（GroupPicker）+ 图标选择 + 代理选择
├── views/group_picker.rs（新）  可搜索下拉 + 新建分组
├── views/icon_picker.rs（新）   预设图标网格（对话框与右键复用）
└── views/main_window.rs         菜单动作分发（三类独立枚举）+ 顶部布局接入
```

### 数据模型 v2

```jsonc
{
  "version": 2,
  "saved_at": "2026-11-19T…",
  "folders": ["生产", "生产/数据库", "测试"],   // 显式分组：支持空分组、重命名
  "sessions": [
    {
      "id": "…",
      "name": "web-01",
      "icon": "🐧",                             // 新增，可选；None = 默认 🖥️
      "folder": "生产/数据库",
      "proxy_jump": { /* 已有字段，本次接入 UI 与连接 */ },
      "port_forwards": [                        // 新增，可空
        {
          "id": "…",
          "kind": "local",                      // local | remote | dynamic
          "bind_host": "127.0.0.1",
          "bind_port": 13306,
          "target_host": "127.0.0.1",
          "target_port": 3306,
          "auto_start": false
        }
      ],
      …
    }
  ]
}
```

- **兼容策略**：`#[serde(default)]` 读取 v1；首次保存写 v2；v2 不被旧版本识别（旧版会拒绝更高 `version`，文档注明不支持降级）。
- **分组模型**：仍以 `folder` 路径作为唯一键，不引入 Group ID；重命名 = 批量更新前缀（`生产` → `生产环境` 时同步 `生产/数据库` → `生产环境/数据库`）。
- **图标**：只存预设 emoji 字符串，不引入图片资源管线；非法/缺失回退默认图标。

### SSH 代理连接链

```
connect(session)
  ├─ build_chain(session) → [跳板A, 跳板B, 目标]（环路/深度校验）
  ├─ 逐跳：TCP → handshake → known_hosts(TOFU) → auth → 保存 ssh2::Session
  ├─ 下一跳：上一跳.channel_direct_tcpip(host, port) 作为 set_tcp_stream
  └─ 目标会话认证成功后，所有跳板 Session 由 SshSession 持有至 disconnect
```

- **错误定位**：任一跳失败时错误信息标注「经跳板 `<名称/主机>` 连接失败」，不吞掉底层原因。
- **超时**：每跳沿用该会话自身的 `connect_timeout_secs`；扫描/传输逻辑不变。
- **UI**：会话编辑「代理」区块：`无 / 选择已有会话`（下拉排除自身与将形成环路的会话）+ 链式预览。
- **Out of scope**：`ProxyCommand`、SSH 证书、跳板机单独认证弹窗（复用现有主机密钥三选一）。

### 右键菜单职责划分

| 目标 | 菜单分区 | 动作 |
|------|----------|------|
| 空白区 | 新建 | 新建会话、新建分组、导入会话… |
|  | 视图 | 展开全部、折叠全部 |
| 分组 | 新建 | 在此分组新建会话、新建子分组 |
|  | 管理 | 重命名分组、删除分组（仅空分组） |
|  | 传输 | 导出该分组… |
| 会话 | 打开 | 打开终端、打开 SFTP、打开监控 |
|  | 编辑 | 编辑会话、更改图标…、移动到分组 |
|  | 管理 | 复制会话、删除会话（危险色 + 二次确认） |

- 状态统一为 `ContextMenuTarget { Blank, Folder(String), Session(Uuid) }`；
  动作拆为 `BlankMenuAction / FolderMenuAction / SessionMenuAction`，由 `MainWindow` 按类型分发。
- 菜单项、分隔线、位置夹取（不出窗口）、Esc / 点击外部关闭抽到 `views/context_menu.rs`；
  SftpView 现有菜单暂不迁移，预留后续统一。

### 顶部菜单栏

- **布局**：主窗口顶部新增 26px 自绘菜单栏（位于 Tab 栏之上），左侧菜单、右侧保留窗口控制区，保留拖拽区域。
- **菜单结构**：

| 菜单 | 菜单项 |
|------|--------|
| 会话 | 新建会话、新建分组、导入会话…、导出会话…、设置… |
| 连接 | 打开终端、打开 SFTP、打开监控、打开端口转发…（依赖当前选中会话，无选中时禁用） |
| 视图 | 显示/隐藏会话侧栏、展开全部、折叠全部、切换主题、增大字号、减小字号 |
| 帮助 | 快捷键、打开日志目录、关于 |

- **交互**：点击或 `F10` 展开；展开后悬停切换菜单、方向键导航、Enter 执行、Esc / 点击外部关闭；
  菜单展开时屏蔽全局快捷键（复用 `blocks_global_shortcuts` 机制）。
- **复用**：下拉菜单直接复用 `views/context_menu.rs` 的菜单项 / 分隔线 / 位置夹取组件；
  动作通过 `AppEvent::MenuCommand(MenuCommand)` 经 EventBus 分发，MainWindow 映射到现有 `request_*` 方法。
- **上下文**：连接菜单依赖会话树选中态；若现有树未维护，补 `selected_session: Option<Uuid>` 并随选中变化发事件。
- **跨平台**：自绘菜单栏，不依赖系统原生菜单，保证 Windows / Linux / macOS 行为一致。

### SSH 端口转发

- **范围**：v1 支持本地转发 `-L` 与远程转发 `-R`；动态转发 `-D`（SOCKS5，无认证 CONNECT）作为可选延伸任务；不做 UDP 转发与 SOCKS 认证。
- **模型**：`Session.port_forwards: Vec<PortForward>`，字段含 `kind / bind_host / bind_port / target_host / target_port / auto_start`；校验端口范围、必填目标、同会话内监听端口不冲突。
- **运行时**：新 crate `rterm-forward`，每条启用规则使用**独立 SSH 连接**，不与终端 / SFTP / 监控共享 Session（规避 libssh2 并发限制与相互阻塞）；连接同样支持 `proxy_jump` 跳板链。
- **Worker 模型**：规则线程持有连接与监听器，状态 `Starting / Running / Error / Stopped` 经 channel 回 UI；停止时先关闭监听、再断开连接，确保端口释放。
- **转发实现**：
  - Local：本地 `TcpListener` accept → `channel_direct_tcpip(target)` → 双向字节泵；
  - Remote：`channel_forward_listen` accept → 连接本地目标 → 双向字节泵；
  - Dynamic（可选）：内置最小 SOCKS5（无认证、仅 CONNECT）→ `channel_direct_tcpip`。
- **UI**：会话右键 / 菜单栏「连接 → 端口转发…」打开 `PortForwardView` Tab；列表展示类型、监听地址、目标、自动启动、状态与错误；支持启停、增删改、全部停止。
- **安全与边界**：默认绑定 `127.0.0.1`，绑定非回环地址时二次确认；会话关闭 / 应用退出时统一停止全部规则；远程转发失败（服务端 `AllowTcpForwarding`/`GatewayPorts` 限制）时透传可读错误。

---

## 任务拆分

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 13.0.1 | 1h | `rterm-session/src/model.rs` | `Session.icon`、`SessionFile.folders`、`FILE_VERSION=2`、serde 默认值与迁移 | v1 会话文件可直接加载；往返序列化测试通过 |
| 13.0.2 | 0.5h | `rterm-session/src/export.rs` | 导出 schema v2（含 folders/icon/proxy），导入兼容 v1/v2 | v1 导出文件可导入；新字段在导出中保留 |
| 13.1.1 | 0.5h | `rterm-session/src/proxy.rs`（新） | `build_chain` / `detect_cycle` / 深度上限纯函数 | A→B→C 通过；A→B→A 与 >5 跳返回明确错误 |
| 13.1.2 | 1.5h | `rterm-ssh/src/lib.rs` | 代理链连接：`channel_direct_tcpip` + 持有中间 Session + 分跳错误文案 | 自跳板（服务器经自身 `127.0.0.1:22`）终端握手成功 |
| 13.1.3 | 1h | `views/session_dialog.rs` | 「代理」区块：无 / 选择已有会话；环路与自身禁用；链式预览 | 保存后重启仍保留代理配置 |
| 13.1.4 | 0.5h | `views/terminal.rs` / `sftp.rs` / `monitor.rs` | 连接入口无需改动，验证代理链对终端 / SFTP / 监控全部生效 | 三类 Tab 均经代理连通；失败文案含跳板名 |
| 13.2.1 | 0.5h | `views/import_export.rs` / `main_window.rs` | 统一入口（空白菜单 / 设置页）；按分组导出接线 | 从空白菜单与分组菜单均可导出，范围正确 |
| 13.2.2 | 1h | `views/import_export.rs` + `rterm-session/src/openssh.rs` | OpenSSH config 导入 UI：选文件 → 预览 → 冲突策略 | `~/.ssh/config` 示例可导入，`ProxyJump` 写入代理 |
| 13.2.3 | 0.5h | `views/import_export.rs` | 预览展示分组 / 图标 / 代理；导入后树即时刷新 | 预览字段与实际导入一致 |
| 13.3.1 | 1h | `rterm-session/src/store.rs` | 分组 CRUD：`create_folder` / `rename_folder` / `delete_empty_folder` / `list_folders` | 重命名同步前缀；重名/删除非空分组报错；单测覆盖 |
| 13.3.2 | 0.5h | `views/session_tree.rs` | 树合并「显式 folders ∪ 会话派生 folder」，空分组可见 | 新建空分组重启后仍显示 |
| 13.3.3 | 1h | `views/group_picker.rs`（新）+ `session_dialog.rs` | 可搜索分组下拉 + 内联「新建分组…」，替换自由文本 | 选择/新建/过滤可用；非法字符与重名被拦截 |
| 13.3.4 | 0.5h | `views/main_window.rs` | 分组重命名 / 删除 / 移动到分组接线 | 重命名后所有子会话路径同步；删除空分组生效 |
| 13.4.1 | 0.5h | `views/session_tree.rs` | 空白区右键：区分行与空白（`stop_propagation` 或命中测试），发送 `Blank` 菜单事件 | 行右键仍出会话/分组菜单；空白处出空白菜单 |
| 13.4.2 | 0.5h | `views/main_window.rs` | 空白菜单动作：新建会话、新建分组、导入、展开/折叠全部 | 五项均可执行；新建会话默认无分组 |
| 13.5.1 | 0.5h | `views/icon_picker.rs`（新）+ `session_dialog.rs` | 预设图标网格（约 12 个）+ 默认项；对话框内选择 | 选择后预览即时变化 |
| 13.5.2 | 0.5h | `views/session_tree.rs` + `main_window.rs` | 树按 `icon` 渲染；右键「更改图标…」弹出复用选择器 | 修改后树立即更新并持久化 |
| 13.6.1 | 1h | `views/context_menu.rs`（新）+ `main_window.rs` | `ContextMenuTarget` 三态 + 三套动作枚举 + 分发重构 | 删除/导入/设置不再出现在会话菜单；所有动作可达 |
| 13.6.2 | 1h | `views/context_menu.rs` | 分区布局、分隔线、危险色、位置夹取、Esc / 点击外部关闭 | 三类菜单布局符合上表；贴边不出界 |
| 13.6.3 | 0.5h | `views/context_menu.rs` | 抽取共享菜单项 / 分隔线组件，主窗口与树复用 | 无重复渲染代码；现有 Sftp 菜单不受影响 |
| 13.7.1 | 1.5h | 全量回归 | 新建/编辑/代理/端口转发/移动/重命名/删除/导入导出/图标/菜单栏/主题/多 Tab | 手工 smoke 清单全过；`cargo test --workspace` 通过 |
| 13.7.2 | 0.5h | `docs/` + README | 更新 roadmap / planning README / CHANGELOG / 用户文档 | 文档与实现一致 |
| 13.8.1 | 1.5h | `views/menu_bar.rs`（新） | 菜单栏骨架：顶部布局、四个菜单、下拉展开/悬停切换/关闭、F10 与方向键 | 菜单可开关；Esc / 点击外部关闭；主题正确 |
| 13.8.2 | 1h | `views/menu_bar.rs` + `event.rs` | `MenuCommand` 枚举与 EventBus 事件；MainWindow 分发到现有动作 | 新建/导入/导出/设置/终端/SFTP/监控与快捷键行为一致 |
| 13.8.3 | 0.5h | `views/main_window.rs` | 顶部布局接入：菜单栏高度、拖拽区、Tab 栏位置、弹窗层级 | 高 DPI / 缩放不重叠；窗口拖动正常 |
| 13.8.4 | 0.5h | `views/session_tree.rs` + `event.rs` | 选中会话状态与连接菜单禁用逻辑 | 无选中时连接项灰显；选中后可打开三类 Tab |
| 13.8.5 | 0.5h | `views/menu_bar.rs` + `main_window.rs` | 视图菜单接线：侧栏显隐、展开/折叠、主题、字号 | 动作立即生效并持久化 |
| 13.9.1 | 0.5h | `rterm-session/src/model.rs` | `PortForward` 模型与校验（端口范围 / 目标必填 / 端口冲突）+ serde 默认 | 非法端口与空目标被拒绝；序列化往返通过 |
| 13.9.2 | 1.5h | `crates/rterm-forward`（新） | 转发 worker 骨架：独立连接、状态事件、停止位、双向字节泵 | 可启停；线程随规则退出，无残留 |
| 13.9.3 | 1.5h | `rterm-forward` | 本地转发 `-L`，支持多连接并发 | 经本地端口访问远端服务成功（E2E） |
| 13.9.4 | 1h | `rterm-forward` | 远程转发 `-R` 与监听清理 | 经远端端口访问本地服务成功；停止后端口释放 |
| 13.9.5 | 1h | `views/port_forward.rs`（新）+ `main_window.rs` | 端口转发 Tab：规则列表 / 状态 / 启停 / 关闭 Tab 停规则 | 状态实时刷新；Tab 生命周期正确 |
| 13.9.6 | 1h | `views/port_forward.rs` | 规则编辑对话框：类型联动、校验、非回环绑定确认 | 非法输入拦截；保存后持久化并在重启后保留 |
| 13.9.7 | 0.5h | `main_window.rs` + `menu_bar.rs` + `context_menu.rs` | 入口接线（会话右键、菜单栏连接菜单）与 `auto_start` | 三个入口行为一致；连接会话后自动启动规则 |
| 13.9.8 | 1.5h（可选） | `rterm-forward` | SOCKS5 动态转发 `-D` | `curl --socks5` 经隧道访问成功 |

---

## 风险与规避

| 风险 | 影响 | 规避 |
|------|------|------|
| 跳板 Session 生命周期管理 | 中间连接提前关闭导致目标断流 | 由 `SshSession` 按跳板顺序持有，`disconnect` 逆序释放；自跳 E2E 覆盖 |
| 每跳 known_hosts 确认 | 连续弹窗打断连接 | TOFU 行为保持一致；错误/确认文案带跳板主机名；链深上限 5 |
| 会话文件 v2 升级 | 旧版本无法读取新文件 | 保存前自动备份（S10 已有）；文档注明不支持降级；升级前单测覆盖 v1→v2 |
| 分组重命名前缀冲突 | 会话被错误归类/覆盖 | 仅允许重命名到不存在的路径；前缀匹配用 `path == from || starts_with(from + "/")`；单测覆盖 |
| 空白区右键与行右键冲突 | 点行却弹空白菜单 | 行处理内 `stop_propagation`；空白菜单仅由树根容器处理 |
| 菜单重构影响现有入口 | 打开监控/SFTP 失效 | 动作枚举按目标拆分但复用现有 `request_*` 方法；逐项 smoke |
| 菜单栏助记键与终端/输入冲突 | Alt 字母被终端吞掉或误触发菜单 | 以 F10 + 点击为主入口；Alt 助记仅在非终端焦点时启用；菜单展开时屏蔽全局快捷键 |
| 菜单栏挤压顶部布局 | 高 DPI / 小窗口下 Tab 栏错位 | 菜单栏固定高度 + 可拖拽区不变；主窗口使用 flex 纵向布局，回归多分辨率截图 |
| 端口转发与终端会话并发 | libssh2 非线程安全，跨 Tab 共享 Session 可能互相阻塞 | 每条转发规则使用独立 SSH 连接；终端/SFTP/监控 Session 不复用 |
| 本地端口占用 / 权限不足 | 规则启动失败但用户不知原因 | 启动前预检端口；错误透传系统提示；非回环绑定二次确认 |
| 远程转发受服务端限制 | `AllowTcpForwarding no` / `GatewayPorts` 导致监听失败 | 明确展示服务端错误；文档注明需服务端允许 |
| 规则随会话关闭泄漏 | 端口未释放、线程残留 | 会话关闭 / 应用退出统一 `stop_all`；关闭转发 Tab 同步停止规则 |
| 图标 emoji 字体缺字 | 图标显示为方块 | 仅使用常见 emoji；缺字时回退默认图标 |
| S12 并行修改 `main_window.rs` | 合并冲突 | 13.6 菜单重构放在 13.4/13.5 之后，单独提交；与 S12 分支约定先合并顺序 |

---

## 交付定义

- [x] 会话可配置跳板代理（含链式与环路校验），终端 / SFTP / 监控均可经代理连接，错误文案可定位跳板
- [x] 分组可新建（含空分组）、重命名、删除；会话编辑使用可搜索分组下拉
- [x] 会话树空白处 / 分组 / 会话三类右键菜单职责清晰，全局功能不再混入会话菜单
- [x] 顶部菜单栏（会话 / 连接 / 视图 / 帮助）可用，动作与右键菜单、快捷键一致；无选中会话时连接项禁用
- [x] 端口转发支持本地 `-L` / 远程 `-R` / 动态 `-D`，规则可启停与自动启动，状态与错误可视，会话关闭时全部停止
- [x] 会话支持预设图标修改并持久化，导出 / 导入保留图标、分组、代理、端口转发
- [x] 导入导出统一入口，OpenSSH config 可在 UI 中导入
- [x] `cargo fmt --check` / `cargo clippy --workspace --all-targets`（0 warning）/ `cargo test --workspace` 全绿
- [x] 真实验证（2026-11-19，Ubuntu 26.04.1 @ 192.168.1.103）：
  直接 SSH 终端 / SFTP（内嵌侧栏 + 独立 Tab）/ 监控均正常；
  本地转发 `127.0.0.1:13306 → 127.0.0.1:22` 回读 SSH banner；
  远程转发 `远端 127.0.0.1:18080 → 本地 127.0.0.1:13306` 回环回读 SSH banner；
  空分组 `test/empty` 重启保留；顶部菜单、会话/空白右键、图标、模式切换回归通过；
  期间发现并修复「模态弹窗打开时 `focus_pending` 抢回终端焦点」缺陷
- [ ] 跳板链真实验证：需第二台可达跳板主机；链路顺序 / 环路 / 5 跳上限已由单元测试覆盖

---

## 实现备注（2026-11-19）

- **逐跳代理实现**：`ssh2` 的 `set_tcp_stream` 仅接受真实 socket（Windows 需 `AsRawSocket`），
  无法直接传入上一跳的 `Channel`。`rterm-ssh` 采用「本地回环中继」：上一跳
  `channel_direct_tcpip` → `127.0.0.1:临时端口` → 下一跳 `TcpStream`；跳板 Session 与中继线程
  由 `SshSession` 持有，断开时目标先断、跳板逆序释放。
- **schema v2**：`SessionFile.folders` + `Session.icon` + `Session.port_forwards`；
  v1 文件直接加载（`serde(default)`），首次保存写 v2；保存前沿用 S10 的 24h 备份策略。
- **分组**：不引入 Group ID，路径即唯一键；新建自动补父路径，重命名同步前缀，删除仅限空分组。
- **图标**：12 个常见 emoji 预设；编辑对话框内联选择 + 右键「更改图标…」弹窗复用同一网格。
- **右键 / 菜单栏**：`views/context_menu.rs` 统一三态目标、动作枚举、菜单项 / 分隔线 / 分区与位置夹取；
  顶部菜单栏动作经 `AppEvent::MenuCommand` 分发，菜单展开时屏蔽全局快捷键，Esc / 点击外部关闭。
- **端口转发**：`rterm-forward` 每条规则独立 SSH 连接（支持跳板链）；本地 / 动态监听非阻塞轮询，
  远程 `-R` 用 500ms 超时轮询停止位；`-D` 实现最小 SOCKS5（无认证 + CONNECT）。
- **自动启动**：会话首次打开终端时，若存在 `auto_start` 规则且尚无转发 Tab，则在后台创建转发 Tab
  （不抢焦点）并启动规则；关闭最后一个相关 Tab / 应用退出时 stop_all。
- **非回环绑定**：保存规则时二次确认；默认 `127.0.0.1`。

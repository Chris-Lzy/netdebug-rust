# 网络调试助手（Rust 重写）—— 框架版

> **全部用 Rust 写**：egui/eframe 原生界面，没有 HTML/CSS/JS，没有 webview，
> 没有 Tauri。HTML 原型只留作布局参考，不参与实现。
> 旧的 Tauri + HTML 框架已归档在 `~/workspace/netdebug-rust-tauri-archive/`，没有删除。

## 状态：框架版 + 部分真实后端

四个页面都已搭好，后端接了一部分（其余 stub 点下去弹”未接后端”）：

| 页面 | 状态 |
|---|---|
| 调试 | 6 种协议切换字段、配置档案、日志、原始/Modbus 发送区；**地址栏历史记录+下拉**是真的；连接/发送是 stub |
| 主站 | Modbus TCP 主站：**真实轮询/单次读写**（01/02/03/04/05/06/15/16、异常码中文报错、结果表+请求日志）；RTU 串口是 stub |
| 从站 | 从站地址、TCP 502/RTU 串口、四区数据表可在线改值、请求日志；仿真是禁用的”待定”区 |
| 网卡 | **适配器枚举和当前 IP/掩码/网关/DNS 是真实读取**（PowerShell，只读不提权）；档案增删改+保存 config.json 是真的；应用 IP/恢复 DHCP/重命名（netsh，需管理员）是 stub |

## 模块地图

```
src/
├── main.rs        # 入口：eframe 原生窗口 1280×800（最小 480×320），首帧自适应屏幕，深色主题
├── app.rs         # 外壳：顶栏 + 页签栏 + 内容区；页签可拖出成独立窗口再收回
├── pages.rs       # ★ 页面注册表：加页面只改这里一行
├── pages/
│   ├── （子模块在 pages.rs 顶部声明）
│   ├── debug.rs   # 调试页
│   ├── master.rs  # 主站页（Modbus TCP 主站）
│   ├── slave.rs   # 从站页
│   ├── nic.rs     # 网卡页
│   └── template.rs# 示例页（模板，默认不显示）
├── modbus.rs      # Modbus TCP 帧打包/解析 + 后台轮询线程
├── nic_backend.rs # 网卡只读枚举（PowerShell Get-NetAdapter，后台线程）
├── config.rs      # config.json（exe 同目录）：损坏自动备份 .bak 再回默认值，原子写盘
├── log.rs         # 有界环形日志（默认 3000 条，满了丢最旧并计数）
├── bus.rs         # 事件总线（有界 mpsc，页面与后台任务互不认识）
└── industrial.rs  # 工业辅助：确定性定时器、看门狗、有界通道
```

## 加一个新页面

见 [docs/ADD_PAGE.md](docs/ADD_PAGE.md)：复制模板、加一行注册、编译运行，三步。

## 在 Windows 上构建（已验证）

```bat
:: 1. 装 Rust stable（https://rustup.rs/）
::    无 MSVC 的机器选 x86_64-pc-windows-gnu 工具链
:: 2. GNU 工具链补 binutils（raw-dylib 链接需要 dlltool + as）：
::    把 MSYS2 mingw64/bin 加进 PATH（本机已永久配置）
:: 3. 进项目目录
cargo run                 :: 调试运行
cargo build --release     :: 出 exe，target\release\netdebug.exe
cargo test --release      :: 19 个测试
```

高 DPI 小屏注意：窗口首帧会自动缩回屏幕内（`app.rs::fit_window_to_screen`），
250% 缩放的 4K 屏实测完整可见。

打包时要补两件事（框架阶段还没做）：

- 应用图标：发布前放一个 `netdebug.ico`，在 `main.rs` 里用
  `ViewportBuilder::with_icon(...)` 挂上；
- 网卡提权：发布版要在 exe 清单里加 `requireAdministrator`
  （原版 nettool 就是这么做的，启动弹一次 UAC）。

## 验证实话（2026-11-16，Linux 虚拟机）

| 检查 | 结果 |
|---|---|
| `cargo check` | ✅ 通过，0 警告（2026-11-16 晚）。第一次跑失败过：24 个错，全是代码问题（egui 0.37 API 变了，见下），不是环境问题；已逐个改掉再跑通。 |
| `cargo test` | ✅ 19 通过、0 失败（config 3、log 3、industrial 4、nic_backend 3、modbus 6，含 Modbus TCP 假从站回环集成测试）。 |
| `cargo fmt --check` | ✅ 通过。 |
| Windows 构建 / 运行 | ✅ **2026-11-16 已在 Windows 11（250% DPI）实测**：GNU 工具链编译 0 警告，四页 GUI 截图验证完整可见，网卡页读到真实 WLAN IP。过程见 `开发记录.md`。 |
| 无 HTML/CSS/JS/webview/Tauri | ✅ 项目里一个 `.html`/`.css`/`.js` 文件都没有；`src/` 里 grep `tauri|webview` 只剩 main.rs 里一句“没有 Tauri”的注释。 |

**egui 0.37 的 API 变化（写框架时踩到的坑，后人注意）：**

- `eframe::App` 没有 `update` 了，改成 `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)`；`on_exit` 只有 `&mut self` 一个参数（glow 特性默认没开）。
- `TopBottomPanel` / `SidePanel` 没了，统一成 `egui::Panel::top/bottom/left/right(id).show(ui, …)`；`CentralPanel` 还在。
- `ComboBox::from_id_source` 改名 `from_id_salt`；`Response::drag_released` 改名 `drag_stopped`；`SyncSender` 才有 `try_send`（`Sender` 没有）。

## 许可证

MIT，见 [LICENSE](LICENSE)。欢迎 Star / Issue / PR。

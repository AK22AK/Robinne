# Robinne

一个面向 macOS / Windows 的本地优先桌面任务/记录应用，使用 GPUI 构建，数据默认保存在本地 SQLite。

## 当前能力

- 看板首页：汇总任务、记录和统计信息，支持从看板直接进入详情
- 多面板工作区：看板、任务、记录、时间线、搜索、设置
- 时间线：使用 `~时间线` 或 `~项目/时间线` 把任务/记录归入正在跟踪的事情
- 快捷输入：支持全局快捷键呼出，任务/记录双模式快速录入
- 任务语法：支持 `!!`、`!` 标记优先级，支持 `#标签` 和 `@人物`
- 附件支持：任务和记录可添加附件，并在详情侧栏中查看
- 数据管理：查看容量概览、附件健康状态，并执行导入/导出
- 搜索与筛选：支持全文搜索，以及按标签/人物筛选
- AI 面板：支持按时间范围、标签、人物生成“过去总结”或“未来提炼”

## 技术栈

- UI 框架：[GPUI](https://github.com/zed-industries/zed)
- 组件库：[gpui-component](https://github.com/longbridge/gpui-component)
- 数据存储：SQLite（`rusqlite`）
- 异步运行时：Tokio

## 平台与依赖说明

- 当前正式支持 macOS 和 Windows
- 需要本地 Rust/Cargo 环境
- 仓库当前已启用 `gpui_platform` 的 `font-kit` feature；如果后续调整依赖配置，需继续保留该 feature
- macOS 保留原生文件选择和 Quick Look 预览；Windows 使用系统默认应用打开附件预览

详见 [docs/DEBUGGING_TEXT_RENDERING.md](docs/DEBUGGING_TEXT_RENDERING.md)。

## 快速开始

```bash
# 克隆项目
git clone <repository-url>
cd Robinne

# 开发运行
cargo run

# 运行测试
cargo test

# 打包 macOS App / ZIP / DMG
./build_mac_app.sh

# Windows 打包在 Windows 环境执行
pwsh ./scripts/package_windows.ps1
```

当前对外分发建议：

- macOS 优先使用 ZIP 包中的 `Robinne.app`
- `.dmg` 当前仅用于验证构建链路，在补齐 codesign / notarization 前不作为推荐分发方式

## 使用说明

### 快捷输入

- 通过全局快捷键呼出快捷输入窗口，支持“任务”和“记录”两种模式
- 以下示例中的主修饰键：macOS 为 `Cmd`，Windows 为 `Ctrl`
- 任务模式下：
  `Enter` 换行补充正文，`Cmd+Enter` 保存，`Shift+Cmd+Enter` 保存并打开任务面板
- 记录模式下：
  `Enter` 换行补充正文，`Cmd+Enter` 保存，`Shift+Cmd+Enter` 保存并打开记录面板
- 快捷输入支持添加附件

### 任务与记录语法

- `!! 任务内容`：高优先级任务
- `! 任务内容`：普通优先级任务
- `任务内容`：普通录入，不额外设置优先级语法
- `!! 跟进方案 #工作 @张三`：高优先级任务，同时带标签和人物关联
- `记录内容 #会议 @李四`：创建记录时同样支持 `#标签` 和 `@人物`

### 标签、人物与筛选

- 使用 `#标签名` 标记任务或记录，例如 `#工作 #紧急`
- 使用 `@人物名` 关联相关人物，例如 `@张三 @李四`
- 在看板、任务列表、搜索等场景可按标签或人物继续筛选内容

### 默认快捷键

以下为当前默认值：

- macOS
  - 全局快捷键：`Cmd+Shift+T`、`Cmd+0`、`Cmd+2`、`Cmd+3`
  - 应用内快捷键：`Cmd+1`、`Cmd+2`、`Cmd+3`、`Cmd+4`、`Cmd+5`、`Cmd+K`、`Cmd+,`
- Windows
  - 全局快捷键：`Ctrl+Shift+T`、`Ctrl+0`、`Ctrl+2`、`Ctrl+3`
  - 应用内快捷键：`Ctrl+1`、`Ctrl+2`、`Ctrl+3`、`Ctrl+4`、`Ctrl+5`、`Ctrl+K`、`Ctrl+,`

当前 README 仅记录默认行为，不表示这些快捷键已经具备完整的持久化自定义配置能力。

## 项目结构

```text
Robinne/
├── src/
│   ├── main.rs                 # 应用入口、窗口装配、菜单与快捷键注册
│   ├── lib.rs                  # 模块导出与基础测试
│   ├── platform/               # 平台服务层（文件对话框、预览、通知、菜单、快捷键）
│   ├── store.rs                # 状态层与异步命令分发
│   ├── db.rs                   # SQLite 数据访问
│   ├── data_management.rs      # 数据导入导出与附件归档逻辑
│   ├── config.rs               # 快捷键默认配置
│   ├── models.rs               # 数据模型
│   └── ui/                     # 各功能面板与 UI 组件
├── docs/
│   ├── DEBUGGING_TEXT_RENDERING.md
│   └── plans/
├── assets/                     # 图标与静态资源
├── scripts/                    # 打包与资源生成脚本
├── build_mac_app.sh            # macOS App 打包脚本
├── scripts/package_windows.ps1 # Windows ZIP 打包脚本
└── Cargo.toml
```

## 相关文档

- [docs/DEBUGGING_TEXT_RENDERING.md](docs/DEBUGGING_TEXT_RENDERING.md)：GPUI 文字渲染问题调试记录
- [docs/platform-compatibility.md](docs/platform-compatibility.md)：平台差异、降级策略与跨平台开发约定
- [docs/plans/roadmap.md](docs/plans/roadmap.md)：项目路线图
- [docs/plans/product-design.md](docs/plans/product-design.md)：产品设计说明
- [docs/plans/ui-ux-design.md](docs/plans/ui-ux-design.md)：UI / UX 设计文档
- [docs/plans/data-model.md](docs/plans/data-model.md)：数据模型设计

## 许可证

MIT

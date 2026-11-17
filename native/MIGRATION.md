# Rust + Slint 迁移验收

行为、交互、配色和配置基线为正式版 v1.0.5（51de3c8），附件中的 v1.0.4 基线由此替换。已删除的固定本地路径迁移与 docs 目录不恢复。

先在同仓独立分支验证 Windows x64 单文件、中文、整窗透明、托盘、主窗与盘口的首帧及同机资源使用；随后按既有调用链移植数据和各页面。所有结果记录实际环境，不把 Linux 测试、模拟回放或单台 Windows 结果外推为两套干净系统验收。

- 自选代码/首字母搜索、显式选择、批量去重与排序保持一致。
- 腾讯/东财的数据契约、价格精度、手数与金额单位保持一致；新浪仍未接入。
- 行情、图表、成交独立取消及丢弃迟到响应；隐藏后停止请求，恢复立即刷新。
- 分时、日 K、五档、分笔、缓存时长、刷新频率、交易时钟保持一致。
- 黑色深红界面、22 像素菜单、字号、透明度、置顶和快捷键冲突处理保持一致。
- PascalCase settings.json 数值枚举及快捷键位掩码兼容；保存原子替换并备份。预览使用独立配置目录和实例标识。
- 更新保留官方来源、大小、SHA-256、产品身份和版本核验；原生 EXE 不经旧 .NET 更新器直接投递。正式更新发布前落实过渡路径。
- 用相同回放逐字段对照显示与交互，再做真实源验证和 Windows 性能测量。

正式版和安装目录保持现状，预览不触发现有 v* 正式发布工作流。

## 1.0.6-preview.3 桌面预览

自选列表按股票名、价格、涨跌幅和成交量安排三段近似等宽的留白。五档和成交明细的价格列位于档位或时间与数量之间，依据常见字形宽度平衡两侧间距。价格和手数分列右对齐，末列对齐各表右边缘。分笔成交按时间倒序显示最近 10 笔，与五档买卖的 10 行并排对齐；使用分钟成交时也显示最近 10 条。图表纵轴的价格刻度靠绘图区左边缘右对齐。鼠标停在股票名上约 180 毫秒后切换未固定的盘口，单击选中行后也能继续悬停查看其他股票。初始窗口沿用预览一的尺寸，默认字号下，20 只自选的小窗为 550×564 像素，盘口为 960×840 像素。

## 本地复现

在 Windows x64 安装 Rust MSVC 工具链及 Visual Studio C++ 构建工具，进入本仓库的 `native` 目录。构建机器需要这些工具，预览 EXE 的运行依赖另行按目标系统验收。

```powershell
cargo test --locked --no-default-features
cargo build --locked --release --target x86_64-pc-windows-msvc
$exe = (Resolve-Path .\target\x86_64-pc-windows-msvc\release\LiteTicker-preview.exe).Path
$fixture = (Resolve-Path .\bench\replay.json).Path
$out = Join-Path $env:TEMP ("LiteTicker-ui-" + [guid]::NewGuid())
& $exe --replay $fixture --verify-replay $out | Out-Host
Get-Content (Join-Path $out 'result.txt')
```

`--replay` 使用同一份合成行情，跳过联网、配置读写和全局快捷键。验收驱动保存行情、分时、日 K、设置和自选搜索五个窗口的截图，检查 20 行数据、切换成交显示单位、保存字号、显式选择、隐藏停刷和恢复首刷。启动真实行情时，预览首次从当前用户已有的 v1 设置复制自选；后续保存到 `%LOCALAPPDATA%\LiteTicker-preview\settings.json`。

默认使用本机短测资源占用较低的软件渲染器，中文使用系统字体。对照 FemtoVG/WGPU 时指定独立输出目录。

```powershell
cargo build --locked --release --no-default-features --features renderer-femtovg-wgpu --target x86_64-pc-windows-msvc --target-dir target-wgpu
cargo test --locked --no-default-features --test live_market -- --ignored --nocapture
```

旧版基准入口使用标签 `v1.0.5` 的产品源文件与图标，以相同的 1 秒节奏输入相同的 20 只报价。用 Windows PowerShell 在本地磁盘运行。以下为每版三轮、每轮预热 120 秒后采样 600 秒的命令；快速筛查可传 `-WarmupSeconds 20 -SampleSeconds 60 -Repeat 1`。

```powershell
$baseline = Join-Path $env:TEMP ("LiteTicker-baseline-" + [guid]::NewGuid())
.\bench\build-baseline.ps1 -OutputDirectory $baseline
.\bench\measure.ps1 -BaselineExe (Join-Path $baseline BaselineReplay.exe) -NativeExe $exe -Fixture $fixture -RunUi
```

## Windows 11 同机筛查

2026-11-17，Windows 11 家庭中文版 10.0.26200，Intel Core Ultra 5 125H（14 核、18 逻辑处理器），DPI 192。回放文件 SHA-256 为 `ab1b16a63c247630ad5c8e3513cf79087a0773c179d6f418669724293e194f4b`。每版单独打开行情小窗，预热 20 秒后采集 60 个样本。旧版自身窗口记录为 550×564；旧版的第一版外部观察工具受 DPI 虚拟化影响，记录为 275×282，后续工具已使用物理像素。新版本均验证了 550×564 和报价刷新。下表区分了私有工作集、Private Bytes 和总工作集，单位均为 MiB。

| 版本 | 专用工作集中位数 | Private Bytes 中位数 | 总工作集中位数 | 约一分钟内 CPU 秒数 |
|---|---:|---:|---:|---:|
| v1.0.5 WinForms 回放 | 17.72 | 38.88 | 59.99 | 0.406 |
| Rust / Slint 软件渲染 | 5.05 | 5.87 | 26.87 | 0.375 |
| Rust / Slint FemtoVG + WGPU | 59.08 | 85.98 | 121.41 | 0.203 |
| Rust / Slint Skia | 138.04 | 185.65 | 203.93 | 0.266 |

软件渲染短测的专用工作集下降 71.47%。其 Windows x64 构建静态链接 CRT。Skia 对照使用动态 CRT，导入 MSVCP140 和 VCRUNTIME140。CPU 在短窗口内只有不到半秒的增量，该表无法确定 CPU 排名。短测时各 Windows EXE 的 SHA-256 如下。

| 构建 | SHA-256 |
|---|---|
| WinForms 回放程序 | `f677f8e03710724cf486bd100ef69355202e0aa1c61512a1802e7c34f34518db` |
| 软件渲染 | `748a453b0aeec9b93ac33ecf647ff0a4e33096395d602396428e495ad65aee6d` |
| FemtoVG + WGPU | `119fcaad47fa43604355014996a5ad45b07e49e979646533b773c9f36bdbe48f` |
| Skia | `20b4971107386131e84796621c7e52c4b62e5e17e0ac6fe184b584e80a47b938` |

同日通过 Rust 客户端实测，腾讯对 600519.SH 返回报价、110 个当日分时点、60 条日 K 和 20 条分笔；按编号和 `gzmt` 的搜索都命中了贵州茅台。本机访问东财分笔端点时连接提前关闭。行情服务的可用性以当时的网络和上游响应计。

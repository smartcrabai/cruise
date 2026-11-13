[English](../en/configuration.md) | [日本語](../ja/configuration.md) | [简体中文](../zh/configuration.md) | [返回目录](README.md)

# 配置参考

## 应用配置

Cruise 将应用设置与工作流 YAML 分开存储，应用设置保存在 `$XDG_CONFIG_HOME/cruise/config.json` 或 `config.yaml` 中。如果两者都存在，则优先使用 JSON。有关配置文件的选择规则和其他设置，请参阅 [README 配置参考](cli-reference.md#cruise-config)。

<a id="ui-language"></a>
### 界面语言

在应用配置中设置 `uiLanguage`，或使用 `cruise config --set-language ja`。输入时也接受别名 `ui_language`。可显式指定的语言标签有 `en`、`zh-CN`、`es`、`fr`、`pt-BR`、`ja` 和 `ko`，支持大小写以及 `_`/`-` 的变体。`auto` 表示未指定；`cruise config --set-language auto` 会清除已保存的设置。设置或验证应用配置时，无效值会被拒绝。

对于 CLI 和 TUI，语言选择的优先级依次为 `CRUISE_UI_LANGUAGE`、`uiLanguage`、系统区域设置，最后回退到英语。环境变量值为空或为 `auto` 时，会使用下一优先级的设置。无效的环境变量值会触发英文警告并被忽略。系统区域设置检测会按顺序使用 `LC_ALL`、`LC_MESSAGES`、`LANG` 或 `LANGUAGE` 中第一个非空的值；如果该值不受支持，也不会尝试后续变量。如果无法通过环境变量确定区域设置，Windows 还会查询用户区域设置。macOS 和 Linux 仅使用环境变量，因此通过 GUI 启动时，如果没有这些环境变量，则默认使用英语。

如果浏览器中的 `cruise_ui_language` Cookie 有效，WebUI 就会使用它，否则使用服务器进程的默认语言。在 WebUI 中选择语言会保存 Cookie 并重新加载页面，不会更改应用配置。界面语言独立于工作流的 `languages.pr` 和 `languages.plan`，这两项设置用于控制生成的提示词。

<a id="config-file-resolution"></a>
## 配置文件解析

cruise 按以下方式解析工作流配置：

1. **`-c/--config` 标志** -- 优先级最高。指定的文件必须存在，否则 cruise 会报错退出。不会显示提示。`-c __builtin__` 选择内置的 `tdd` 工作流，等同于 `-c builtin:tdd`；`-c builtin:<name>` 选择目录中指定名称的工作流。旧的选择器 `builtin:default`、`builtin:simple` 和 `builtin:review` 不受支持。固定的工作流目录如下：

   | 名称 | 描述 |
   |------|-------------|
   | `tdd` | 采用测试先行的实现方式，随后执行“并行分析 → 汇总 → 应用”验证组和 PR 工作流 |
   | `refactor` | 以差异为起点改进代码、测试和结构，并进行验证 |
   | `add-tests` | 审核行为和用户流程的覆盖情况，添加有意义的测试和 CI，然后独立验证并补齐缺口 |
   | `add-formal-specs` | 添加可执行的 Quint、Alloy 和 Gherkin 规范并接入 CI；修复规范/源代码中的实际缺陷 |
   | `research` | 在限定范围内开展只读调查，提供有来源依据的证据、独立审查和附有引用的报告 |
   | `security` | 限定范围内的安全盘点与并行审计、问题分诊、对已确认缺陷的安全修复，以及独立验证 |
   | `pr-review-fix` | 在现有 GitHub PR 的头分支上处理符合条件的审查讨论串 |

   `refactor`、`add-tests`、`add-formal-specs` 和 `security` 默认设置 `force_exec: true`：本地规划直接在当前目录中运行，不使用外层计划/会话或工作树，也不会自动创建 PR。传入 `--no-force-exec` 可改用常规的计划/会话流程。

   `cruise workflow list` 会输出工作流目录。要自定义内置工作流，请使用 `cruise workflow eject <name> [--to user|project]`，例如 `cruise workflow eject tdd --to project`。导出操作仅接受目录中的名称，并且绝不会覆盖已有文件。

   `refactor` 以仓库中现有的已暂存、未暂存和未跟踪更改为起点，并在存在可信的基准/合并基准时，纳入任务分支相对于该基准/合并基准的更改。它将更改限制在该差异及必要的调用方/依赖项之内；如果没有差异，则将范围设为整个仓库。现有工作会被保留，不会对无关编辑执行重置、贮藏或还原。

   `add-tests` 盘点生产代码的行为和真实用户旅程，将稳定的覆盖项/缺口 ID 映射到源代码边界和测试断言，然后添加或补全有用的测试及适用的 CI。`add-formal-specs` 添加可执行的 Quint 和 Alloy 模型，以及带有与生产代码相连的步骤定义的 Gherkin 场景，在 CI 中运行它们，并修复反例或实际的源代码/规范缺陷，而不弱化要求。

   仅靠步骤顺序并不会将前序报告插入提示词。`add-formal-specs` 将审计和模型输出保存为会话产物（`formal-specs/audit.md` 和 `formal-specs/model.md`），其后续提示词会通过 `{file:...}` 显式嵌入所需的报告内容。

   `add-tests` 将原始请求（`{input}`）和通过 `{file:test-ci-gap-plan.md}` 提供的已保存计划传入实现阶段；验证阶段会独立使用 `{input}`、`{file:test-ci-gap-plan.md}` 和 `{file:test-ci-implementation.md}`，对照实际测试、断言和命令，重新检查每个计划处理/已处理的 ID。它会写入 `test-ci-gap-plan.md`、`test-ci-implementation.md`（按 ID 记录 applied/already-covered/rejected/blocked 处理状态及其证据）和 `test-ci-verification.md`。覆盖台账会将行为标记为 `Covered`、`Partial` 或 `Missing`，记录排除项和未经审查的区域及其原因，并对可处理的缺口排定优先级，不强加数值化的覆盖率配额。
   内置的 `tdd`、`refactor` 和 `add-tests` 工作流会遵循仓库说明，并首先为范围内的每个真实产品目标复用合适的既有工具——包括 Windows、Linux 和原生目标——保留其版本、命令、环境设置和 CI。下列建议仅针对适用但缺失的能力；它们不是允许列表、迁移方案、强制安装项、运行时依赖项或新的工作流/配置键。Maestro 等既有工具仍然有效。使用技术栈支持的兼容版本，不要强制引入新的平台测试矩阵。

   针对后端和命令行能力的建议：

   - **可随用随弃的外部依赖：** [Testcontainers](https://testcontainers.com/)（及其 [Rust 库](https://rust.testcontainers.org/)）用于在需要时，利用技术栈支持的库创建可随用随弃的真实外部数据库或依赖服务。锁定兼容的库版本和镜像版本；等待服务就绪；使用动态映射的连接信息、真实的应用迁移、隔离的种子数据，并将清理范围限制在测试自身拥有的资源内。它不是数据库断言框架，对于进程内/嵌入式数据库或没有数据库的项目并无必要。如果所需的 Docker 兼容运行时不可用，应报告为 blocked——不能改用宿主机上的生产数据库、模拟实现，或假装测试通过。
   - **CLI 和 TUI：** [Bats-core](https://github.com/bats-core/bats-core) 可以保存 Bash 测试，这些测试运行实际构建出的 CLI 可执行文件，并对其退出状态、stdout、stderr 和副作用进行有意义的断言。Bats 的 `run` 在捕获命令的执行结果后会返回成功，因此必须显式断言目标命令的状态；仅使用 `run` 并不代表测试通过。对于真正的交互式 TUI，应使用 Bats 编排一个真正支持 PTY 的交互测试框架（复用现有框架，例如在适用时使用 Expect），发送按键，对终端屏幕/状态转换作出断言，并在适用时检查渲染出的终端/图像证据。管道、非交互式 stdout 或 `--help` 冒烟测试都不是 TUI E2E。如果所需的 PTY/运行时不可用，应报告为 blocked。遵守 Bash 和宿主平台的支持范围；不要在原生 Windows 上强行使用不受支持的 Bash。
   - **Web HTTP API：** [Hurl](https://hurl.dev/) 用于保存 `.hurl` 测试，并通过 `hurl --test` 对真实的本地应用 HTTP 端点运行这些测试，对响应及相关状态/效果作出实质性断言。复用既有的环境设置、隔离措施和本地测试认证；在适用时使用 Testcontainers 创建数据库依赖项。静态模式/解析器检查和模拟 HTTP 都不是真实的端点测试；不要使用线上凭据或外部生产服务。
   - **后端服务契约：** [Pact](https://docs.pact.io/)（[Pact 的适用场景](https://docs.pact.io/getting_started/what_is_pact_good_for)）用于 HTTP 或消息契约，但前提是已识别出的真实消费方/提供方参与者可控且受支持。通过实际的消费方客户端代码生成契约，使用显式的提供方状态，针对真实的提供方实现验证这些契约，并记录双方的情况。仅有消费方模拟实现或生成 pact 文件并不能完成验证。Pact 并非通用的后端功能、数据库或集成 E2E 工具，也不能替代 Hurl 或真实路径检查；受控的协议契约替身属于有效契约，而不是仅依赖模拟的 E2E。不要要求使用 Broker/PactFlow、云基础设施或机密信息。

   选择 UI 工具前，先根据清单文件和源代码确定实际产品与框架的类别。对于浏览器目标，仅当相应能力缺失时，才推荐使用 Playwright CLI（`playwright-cli`）进行视觉检查，使用 Playwright Test（`*.spec.ts`、`npx playwright test`）进行持久化 E2E 测试。对于 SwiftUI iOS/macOS，仅在缺少合适的视觉工具时，才推荐使用 Xcode MCP 进行实际应用交互和截图采集（仅有 Preview 并不足够）；仅在缺少合适的、支持保存 UI E2E 测试的工具时，才推荐使用 XCTest/XCUITest（`xcodebuild test`）；只有在确有意图集成时，才额外使用 App Intents Testing。对于 Android，仅在相应的既有能力缺失时，才推荐使用 mobile-mcp 进行视觉检查，使用 Espresso 测试 Views、Compose UI Test 测试 Compose，以及 UI Automator 测试系统/跨应用流程。

   对于 React Native/Expo iOS+Android，仅在缺少相应的合适工具时，才推荐使用 agent-device CLI 或已连接的 MCP 进行视觉检查，使用 Detox + Jest（`detox build`/`detox test`）进行持久化 E2E 测试。Expo 需要自身集成 Detox 的原生构建、锁定的 Expo/RN/New Architecture/Detox 兼容组合，以及其 prebuild/config-plugin 工作流（不能使用 Expo Go，也不能仅手工修改生成文件）；应根据情况使用 iOS Simulator 和受支持的 Android 运行时，而不是一概要求 iOS 真机覆盖。Detox 兼容性不受支持时，应视为 blocked，而不是切换运行器。对于 Electron，仅在缺少相应的合适工具时，才推荐使用 Playwright Electron API 进行真实应用交互，并将 Playwright Test 与该 API 结合用于持久化 E2E 测试；应验证其实验性 API/版本兼容性（在浏览器中运行渲染器不代表原生应用测试通过，OS 对话框桩也不代表真实 OS 对话框覆盖）。

   将 Tauri 归类为原生应用（WebView 加 Rust/IPC），而不是普通的浏览器前端或 Electron 前端。在兼容的宿主机上，如果缺少合适的视觉工具，推荐使用 [tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp) 在真实应用中执行输入操作并截图，随后实际检查图像。如果缺少合适的持久化 E2E 工具，推荐使用 [tauri-webdriver](https://github.com/Choochmeque/tauri-webdriver)，同时配合其必需的配套插件 [tauri-plugin-webdriver](https://github.com/Choochmeque/tauri-plugin-webdriver) 和真正的 WebDriver 客户端/测试运行器（复用现有工具，否则使用与技术栈兼容的客户端，例如 WebdriverIO 或 Selenium）。锁定兼容的版本或 Git 修订版本，并确认宿主机、Tauri、WebView 和客户端之间的兼容性。保存实质性的断言，以及一条独立执行相关真实前端到 Rust IPC/原生行为的命令。这两个插件都仅供开发/测试使用：从生产构建中排除自动化端点，使用本地/隔离的测试会话，不要在全局启用 MCP。应考虑模拟实现、合成/不可信事件和对话框桩的影响；MCP 插件默认会对 JavaScript `alert`/`confirm`/`prompt` 使用桩实现，这不能作为真实原生对话框行为的证据。如果所需的宿主机/运行时/插件能力缺失，应报告为 blocked，而不是以仅浏览器测试通过来替代。

   对于上述框架建议均不适用的原生目标或能力，只能复用合适的既有工具；不要引入新工具。如果已有既定的必需工具，但与其兼容的宿主机、运行时或服务器不可用，应报告确切的阻塞因素。如果视觉交互/截图与持久化 E2E 这两类能力中只有一种可用，应完成能够完成的工作，并如实报告缺失的能力；不要声称已完成两项交付物。对于选中的每个适用 UI 流程，都必须同时满足两方面要求：进行真实应用交互，保存有意义的状态/失败场景的截图并检查其像素内容；将带有断言且具有确定性的 E2E 源代码保存在仓库中，使用重置/隔离的状态，提供精确且可重复执行的构建/运行命令，并进行一次不依赖先前探索会话和 LLM 决策的执行。维护一份可复用的共享证据记录，记录确切的命令/结果和已检查的截图，而不要仅为重复生成证据而重新运行。一次实际的 E2E 运行加上经过检查的截图就足够了；无需重复运行，也无需像素差异/黄金样本测试，仅有视觉证据并不能补齐 E2E 覆盖。如果不存在相关的产品 UI 目标，应如实报告这一限制。受支持宿主机和 CI 上的浏览器/Electron 检查仍在范围之内，且不得禁用 Linux Web CI。

   通过执行智能体提供的机制检查/加载相关的可用技能，并在适用时使用已连接的 MCP 服务器。技能是可选的，单纯缺少技能并不构成阻塞因素；不要添加技能、猜测 API、编辑全局 MCP/配置项，或强制设定 computer-use 选项。如果所需能力缺少合适的既有工具，应采用上文中适用的建议；如果所需的工具/服务器/运行时不可用，应报告确切的阻塞因素，而不是声称成功。

`research` 运行四个范围受限的只读阶段：规划 → 调研 → 独立审查 → 综合整理。它会保留 `research/plan.md`、`research/evidence.md`、`research/review.md` 和 `research/report.md`；每个阶段都携带 `{input}`，后续阶段会通过 `{file:...}` 显式读取先前的报告。最终报告会回答请求，提供有来源依据的事实、单独列出的推断、矛盾之处、不确定性、审查处置结论和确切的阻塞因素。有关外部来源的陈述会引用 URL 和精确定位信息，以及可获得的来源版本/日期和实际检索日期；有关仓库的陈述会引用路径和行号范围。它不会虚构来源或检查。该内置工作流设置了 `create_pr: false`，因此这种持久化会话不会对工作区/代码作任何更改，并会跳过自动提交、推送、PR 创建、`after-pr` 和 PR 清理，同时保留会话产物和工作树；`merge` 绝不会覆盖这一只读/不创建 PR 的策略，已选择启用但不可用的操作会被跳过，并说明原因。请使用 `cruise plan --skip-planning -c builtin:research "question"`，如有提示则批准计划，然后运行 `cruise run <session-id>`——不要使用临时执行方式 `exec` 或 `force_exec`。

   `security` 盘点范围和覆盖情况，并行开展代码/依赖项/交付审计，对发现项进行分类评估，修复已确认且可安全修复的范围内缺陷，并独立验证每个发现项。它会保留 `security/scope.md`、`security/code-findings.md`、`security/dependency-findings.md`、`security/delivery-findings.md`、`security/worklist.md`、`security/repairs.md` 和 `security/verification.md`。每个阶段都会收到 `{input}`；后续阶段会通过 `{file:...}` 显式使用报告，分类评估阶段则以 `{prev.output}` 接收合并后的审计结果。各发现项会保留稳定的 ID，以及有关可利用性、接受/拒绝/受阻等处置结论、误报、修复、回归检查和独立结案的证据；报告会如实指出具体未执行的检查、被排除/未经审查的范围以及局限，而不会宣称整个系统都是安全的。

   research 工作流以及 security 的盘点/审计/分类评估阶段需要使用 SDK 权限进行只读工作。在解析工作流及适用的用户/项目默认设置后，如果 `command:` 和 `sdk:` 均未设置，Cruise 会使用其默认值 `sdk: jcode`；`command:` 后端无法被限制为只读模式，因此会在权限受限的步骤中被拒绝使用。

   交互式选择器（CLI、TUI、WebUI）会将内置工作流和其他候选项按类别组织。没有配置文件时，非交互式解析会使用 `tdd`。

   `create_pr` 是工作流专用的布尔配置项，默认值为 `true`。将其设置为 `false` 会跳过 Cruise 的自动提交、推送、PR 创建、`after-pr` 和基于 PR 的清理，同时保留正常会话、其产物及其工作树；本地不创建 PR 的工作树不需要 `gh`/`glab`，但 `--repo` 仍需要代码托管平台的 CLI 来访问仓库。启用 PR 创建时，如果运行成功但分支上没有新提交，则会跳过 PR 创建而不报错，而分支上已有的提交仍可用于创建 PR；如果未创建 PR，已选择启用的 PR 创建后操作会被跳过，并说明原因。`merge` 绝不会覆盖 `create_pr: false` 或 `research` 这样的只读工作流的限制；显式设置的 `if.no-file-changes` 失败/重试防护规则仍然适用。用户级和项目级 `default.yaml` 文件均不接受 `create_pr`；它不是 CLI 选项。

2. **`CRUISE_CONFIG` 环境变量** -- 如果已设置，则直接使用其指定的文件（文件不存在时报错）。不会显示提示。
3. 否则，cruise 会从以下位置收集所有候选项，并将它们列为可选项：
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml`（当前目录）
   - `./.cruise/*.yaml` / `*.yml`（当前目录），按文件名排序；不包括顶层的 `./.cruise/default.yaml`
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` 以及有效的已锁定捆绑包目录中的成员（默认：`~/.config/cruise/workflows/`），按相对路径排序

   当 stdin 和 stdout 都是 TTY 时，会在分类交互式选择器中显示候选项和内置工作流。在非交互式环境中（通过管道传入 stdin、脚本），会自动选取优先级最高的候选项，而不显示提示。
4. **未找到配置文件** -- 交互式终端会在选择器中提供内置工作流；非交互式解析会自动选择嵌入的 `tdd` 工作流（在源代码树中位于 `builtin/tdd.yaml`）。无需配置文件。

交互式选择器使用配置的 `workflow_categories` 树；如果该树为空或不存在，则使用默认类别：Development（`tdd`、`refactor`）、Quality（`add-tests`、`add-formal-specs`）、Research（`research`）和 Security（`security`）。非空的用户树会替换这些默认设置，而不是与其合并。类别和工作流条目各自保留其声明顺序；CLI 子菜单会先显示子类别，再显示工作流条目。一个工作流可以出现在多个类别中。

不带前缀的条目会匹配文件的主名（不含扩展名的文件名）和内置工作流名称。例如，`refactor` 既匹配 `refactor.yaml`，也匹配该内置工作流，而 `builtin:refactor` 仅匹配内置工作流。来自不同来源的匹配文件都会显示。不存在的名称会被忽略，空类别会被省略。像 `{refactor: "Custom description"}` 这样的条目会为选择器提供描述，并在 WebUI 中覆盖工作流的 `description:`。CLI 选择器不会加载来自文件的工作流描述。同一工作流的描述如果相互冲突，则配置无效；这也包括同一内置工作流的不带前缀名称和带前缀名称所对应的描述。选择器设置会在打开选择器时进行验证，不会阻止运行时模型或并行度默认设置的加载。

未分类的本地候选项和 `CRUISE_CONFIG` 候选项仍可在根级访问（在 WebUI 中位于 `Base dir (...)` 下）。未分类的已安装包显示在 `Packages` 中；其后是未分类的用户目录工作流和内置工作流，位于 `Others` 或由 `others_category_name` 配置的类别中。设置 `show_others_category: false` 会隐藏这些未分类的用户工作流和内置工作流选项，但不会隐藏本地、环境变量指定的或已安装包候选项。WebUI 还会保留其 *Auto* 和内置 `tdd` 选项，并在当前会话的工作流原本会被过滤掉时仍将其保留。类别仅影响交互式显示，不影响显式选择，也不影响非交互式解析的优先级。

<a id="workflow-defaults-defaultyaml"></a>
### 工作流默认值（`default.yaml`）

`$XDG_CONFIG_HOME/cruise/default.yaml` 提供用户默认值。当未设置 `XDG_CONFIG_HOME` 时，在 macOS/Linux 上，路径默认为 `~/.config/cruise/default.yaml`。在 Windows 上，如果设置了 `APPDATA`，则路径为 `%APPDATA%\cruise\default.yaml`；如果缺少 `APPDATA`，Cruise 会回退到主目录下的路径 `~/.config/cruise/default.yaml`。`./.cruise/default.yaml` 提供项目默认值。这些文件为工作流中未设置的字段提供默认值。它们不能被选作工作流（使用 `-c` 或 `CRUISE_CONFIG` 指向它们会报错）。`default` 不是内置工作流名称，因此 `cruise workflow eject default` 会因内置工作流名称未知而被拒绝；`cruise workflow generate --name default` 仍会拒绝写入保留的 `./.cruise/default.yaml`。`<config_dir>/workflows/default.yaml` 仍是普通工作流。

在适用的情况下，优先级从高到低依次为：单次调用的 CLI 标志（包括用于选择工作流的 `-c`） > 已保存会话的清理覆盖设置 > 用于覆盖配置的环境变量 > 工作流文件 > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > 应用程序模型默认值（`config.json` 或 `config.yaml`） > 内置默认值。对于普通提示，步骤级模型覆盖设置仍具有更高优先级。

当实际的 jcode 提示没有生效的模型时，Cruise 会创建 `$XDG_CONFIG_HOME/cruise/default.yaml`（或该平台通常使用的用户配置路径），将其内容设为 `model: null`，并在启动 jcode 之前停止。请编辑该用户级文件，设置一个实际的 `provider/model[:effort]` 引用或单独的模型名称。已有文件不会被修改，也不会生成项目的 `.cruise/default.yaml`。对于新会话，你也可以改为在 `config.json`、`config.yaml` 或工作流中设置 `model`，或通过 `CRUISE_MODEL` 进行设置；规划调用可以使用 `plan_model` / `CRUISE_PLAN_MODEL`。在试运行、草稿、列表查询、仅执行命令或使用 Claude 的情况下，不会创建该文件。更新设置后，请使用发起请求的命令重试：对于运行失败的会话，使用 `cruise run <SESSION>`；对于规划或执行请求，则重新运行相应的 `cruise plan` 或 `cruise exec`。基于文件的会话会重新加载默认值，但已保存的快照会话不会重新应用后续的默认值变更或环境变更；要使用更新后的设置，请启动一个新的快照会话。

- `env` 和 `mcp_servers` 按键合并（较高层级的值优先）。
- `command` 和 `sdk` 构成一对：如果某一层设置了其中任意一项，则忽略所有较低层级中的这一对设置。
- 其他字段（`model`、`plan_model`、`subagent_model`、`max_retries`、`interactive_planning`、`languages`、`cleanup_after_pr`、`force_exec`、`computer_use`、`permission`、`disallowed_tools`、`retry`、`formal_verify`）取自设置了相应字段的最高层级。限制列表在各层级之间采用替换方式；在工作流中显式设置 `disallowed_tools: []` 会清除继承的默认值。
- 工作树设置 `base_branch` 和 `auto_fetch` 是仅限默认配置使用的键，项目值会覆盖用户值。`auto_fetch: true` 要求合并后的 `base_branch` 非空。
- `description`、`steps`、`groups` 和 `after-pr` 只能用于工作流，在 `default.yaml` 中出现会导致加载错误。未知键也会导致错误。
- 如果默认配置指定的后端（`command` / `sdk`）与工作流自身的后端类型不同，那么当这些默认配置会填充尚未设置的后端专属字段 `model`、`plan_model`、`subagent_model` 或 `retry`，添加缺失的 `mcp_servers` 条目，或填充尚未设置的 `computer_use`、`permission` 或 `disallowed_tools` 字段时，无论其值为何，校验都会失败。请在工作流中设置相应字段，或将其从 `default.yaml` 中移除。此检查在应用 `CRUISE_SDK` 覆盖设置之前执行，且不会考虑这些覆盖设置。

`pre_steps:` 定义仅执行命令的步骤（`command`、`env`、`timeout`、`skip`、`when`），这些步骤会以 `pre/<name>` 的形式插入工作流 `steps` 的开头。它们不会在规划阶段或 `after-pr` 中运行。用户步骤排在前面，项目步骤排在后面；与用户步骤同名的项目步骤会在原位置替换该用户步骤。

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```


升级说明：内置工作流不再包含 `mise-trust` 步骤。添加上面的 `mise-trust` 前置步骤即可保留原有行为。

在 GitHub Actions 中，`XDG_*` 是隔离的，因此只有项目的 `.cruise/default.yaml` 会生效。

<a id="config-file-reference"></a>
## 配置文件参考

<a id="basic-structure"></a>
### 基本结构

`formal_verify.model_check_timeout_seconds` 设置 Quint model-check 和 Alloy 的单条命令超时时间，单位为秒。它接受 1 到 86400（含）之间的整数，默认值为 900。可以在工作流 YAML 或分层的 `default.yaml` 中设置：

```yaml
formal_verify:
  model_check_timeout_seconds: 900
```


```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Use jcode-sdk (the fallback only when neither
                          # `command` nor `sdk` remains set after applicable
                          # `default.yaml` layers; see SDK Mode)
# sdk: claude             # Use claude-agent-sdk to drive the claude CLI

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
# In SDK mode, arrays use the first model, then fall back in order.
# model: [sonnet, opus, gpt-5.5]
plan_model: opus          # model used for the built-in plan step (optional)
# subagent_model: haiku   # SDK-only: one subagent model, string or {sdk: claude, model: haiku}
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# commit_message_template: "{title} [{session_id}]" # host-created commit message
# pr_body_template: "## Summary\n{body}"            # generated PR body wrapper
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# permission: full        # prompt permission: read-only | edit | full (default full)
# disallowed_tools: [web, subagent, "shell:npm publish"] # SDK prompt deny list
# computer_use: false     # jcode only, macOS: let prompts control the desktop (macos_computer_use)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

# MCP servers are available to SDK runs only; omit this block for command mode.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

flags:                    # workflow flags (optional; absent values default off, see Workflow flags)
  # run_review: {}

groups:                   # step group definitions (optional)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  step_name:
    # step configuration
    # flag: run_review     # run only when the declared flag is on (optional)

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```


对未知键的处理方式因层级而异：各个步骤映射（位于 `steps`、`after-pr`、组的 `steps` 和并行子项中）以及 `retry` 块会拒绝未知字段，并报解析错误，因此，拼写错误或已移除的字段（如 `fail-if-no-file-changes`）会立即引发配置错误。未知的**顶层**键会被静默忽略，因此文件顶层拼写错误的 `plan_model` 或 `max_retries` 也会被接受，只是不会产生任何效果。

<a id="dynamic-model-selection"></a>
### 动态模型选择

当 `command` 数组包含 `{model}` 占位符时，cruise 会在运行时根据每个步骤实际生效的模型来解析该占位符：

- **已指定模型**（通过顶层 `model` 或步骤级 `model`）：将 `{model}` 替换为模型名称。
- **未指定模型**：自动移除 `{model}` 参数及其前面紧邻的 `--model` 标志。

步骤级 `model` 字段会覆盖顶层 `model` 默认值，但仅对该步骤生效。

此处同样适用模板变量所使用的 Rust `format!` 风格的大括号转义规则（参见[变量参考](steps.md#variable-reference)）：`{{model}}` 表示字面字符串 `{model}`，而不是占位符。任何其他未转义的 `{name}`、空的 `{}`、未闭合的 `{` 或单独出现的 `}` 都属于模板语法错误。

```yaml
command:
  - claude
  - --model
  - "{model}"      # replaced at runtime, or --model/{model} pair is stripped if no model
  - -p

model: sonnet      # default; steps without model: use this

steps:
  planning:
    model: opus    # overrides the default for this step only
    prompt: "Create a plan for: {input}"
```


<a id="prompt-languages"></a>
### 提示词语言

UI 显示语言独立于提示词语言。可通过应用的 `uiLanguage`、`CRUISE_UI_LANGUAGE` 或 TUI 语言选择器进行配置；WebUI 浏览器可以使用自己的语言选择器覆盖服务器默认值。这些设置不会更改 `languages.pr` 或 `languages.plan`。

`languages.pr` 控制自动生成的 PR 标题和正文所用的语言，`languages.plan` 控制 cruise 内置规划提示词所用的语言。`CRUISE_LANGUAGE_PR` 和 `CRUISE_LANGUAGE_PLAN` 会覆盖对应的 YAML 值；空白的环境变量值会被忽略。除此之外，优先使用嵌套字段，其次是已弃用的 `pr_language` / `plan_language`，然后是 `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` 中第一个非空值（仅映射一次；不受支持的值不会继续回退到后续变量），最后是 `English`。已弃用的字段仍作为回退项受支持。

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```


为兼容旧版配置：

```yaml
pr_language: Japanese     # Deprecated; use languages.pr instead.
plan_language: Japanese   # Deprecated; use languages.plan instead.
```


内置模板可通过 `{pr.language}` 和 `{plan.language}` 访问实际生效的值。

<a id="commit-and-pr-templates"></a>
### 提交和 PR 模板

可选的顶层字符串键 `commit_message_template` 和 `pr_body_template` 用于自定义工作树 PR 创建流程中由宿主生成的提交消息和 PR 正文。可在工作流、用户的 `<config_dir>/default.yaml` 或项目的 `.cruise/default.yaml` 中设置它们。优先级为：工作流 > 项目默认值 > 用户默认值 > 未设置。它们不是 CLI 选项，也不是环境变量覆盖项。

```yaml
commit_message_template: "{title} [{session_id}]"
pr_body_template: |
  ## Summary
  {body}
```


| 模板 | 允许的变量 |
|----------|-------------------|
| `commit_message_template` | `{title}`, `{issue}`, `{session_id}`, `{branch}` |
| `pr_body_template` | `{title}`, `{body}`, `{issue}`, `{session_id}`, `{branch}` |

`{title}` 是去除首尾空白后的 LLM 生成的 PR 标题；如果生成的标题为空，则使用会话输入或标题中第一个去除首尾空白后非空的行。`{body}` 是去除首尾空白后的 LLM 生成的 PR 正文；如果生成失败，则为空字符串。`{issue}` 目前始终展开为空字符串，`{session_id}` 是会话 ID，`{branch}` 是工作树分支名。这些模板不会自动添加用于关闭议题的文本。

未知变量和格式错误的语法会在工作流加载/编译时被拒绝。使用 `{{` 和 `}}` 转义字面量花括号；展开后的值不会再次展开。渲染后的正文和提交消息都会去除首尾空白。渲染后的提交消息为空会报错；配置的 Co-authored-by 尾注会另行追加。设置了 PR 正文模板时，Cruise 会显式提供标题和正文，而不使用 `--fill`。

两个模板都未设置时，现有行为保持不变：提交使用生成的 PR 标题或现有的输入回退值，PR 标题为空时使用 `--fill`。这些设置不影响当前分支上的运行、`create_pr: false`、智能体创建的提交或压缩合并消息。

<a id="session-title-generation"></a>
### 会话标题生成

计划获批后，cruise 会生成一个简短的会话标题（最多 80 个字符），在 `cruise list` 和 WebUI 侧边栏中显示，以代替原始任务输入。具体行为取决于后端：

- **SDK 模式（已设置 `sdk:`，或在应用适用的各层 `default.yaml` 配置后，解析后的工作流中 `sdk:` 和 `command:` 均未设置——此时使用默认的 `jcode` 后端）**——在前台 `cruise plan` 的批准流程中，cruise 会使用 `plan_model` -> `model` 单独发起标题生成请求。如果这次 jcode 调用没有实际生效的模型，批准操作会失败，且不会保存该批准。其他标题生成失败的情况会回退到 `plan.md` 元数据。通过 `cruise list`、WebUI/TUI 进行批准时，以及在后台规划时，则从 `plan.md` 获取标题。
- **命令模式（已设置 `command:`）**——不会为生成标题而调用 LLM。标题会自动从生成的 `plan.md` 中的第一个标题或第一个非空行提取。

对于 jcode，请视情况通过工作流、适用的默认配置或 `CRUISE_MODEL` / `CRUISE_PLAN_MODEL` 配置一个实际生效的模型。Claude 工作流和命令工作流在未设置模型时的行为保持不变。

<a id="environment-variables"></a>
### 环境变量

`CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH` 和 `CRUISE_OPENCODE_CLI_PATH` 通过 Cruise 进程环境指定提供商的可执行文件。它们不是工作流配置键，不能通过工作流的 `env:` 提供；请参阅[受管的提供商 CLI](installation.md#managed-provider-clis)。

环境变量可以在工作流级别和步骤级别设置。步骤级别的值会覆盖工作流级别的值，但仅对该步骤生效。并行子项还增加了第三层：工作流 < 并行块 < 子项。值支持模板变量替换；每个子项都基于进入该块前的变量解析自身的值。

进程级配置覆盖会在工作流和默认配置解析完成后应用。支持的名称和值类型如下：

| 变量 | 配置键 | 值 |
|---|---|---|
| `CRUISE_MODEL` | `model` | 模型 |
| `CRUISE_PLAN_MODEL` | `plan_model` | 模型 |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model` | 模型条目 |
| `CRUISE_SDK` | `sdk` | 文本 |
| `CRUISE_COMMAND` | `command` | JSON 数组 |
| `CRUISE_MAX_RETRIES` | `max_retries` | 整数 |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning` | 布尔值 |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr` | 布尔值 |
| `CRUISE_FORCE_EXEC` | `force_exec` | 布尔值 |
| `CRUISE_COMPUTER_USE` | `computer_use` | 布尔值 |
| `CRUISE_PERMISSION` | `permission` | 文本：`read-only`、`edit` 或 `full` |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools` | JSON 数组 |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template` | 文本 |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template` | 文本 |
| `CRUISE_RETRY` | `retry` | JSON 对象或 `null` |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms` | 整数 |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms` | 整数 |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback` | 布尔值 |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains` | JSON 对象 |
| `CRUISE_LANGUAGES` | `languages` | JSON 对象 |
| `CRUISE_LANGUAGE_PR` | `languages.pr` | 文本 |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan` | 文本 |
| `CRUISE_RUN_ALL_PARALLELISM` | 应用的 `run_all_parallelism` | 整数，至少为 1 |

名称通常由 `CRUISE_` 加上大写的键路径组成，并将点和连字符替换为下划线。上述语言子字段的环境变量名称仅支持表中所示的拼写。值会去除首尾空白，仅包含空白的值会被忽略。布尔值仅接受 `true`、`false`、`1` 或 `0`；整数必须由不带正负号的十进制数字组成。文本按字面值处理。模型值按单个模型的字面值处理，除非以 `[` 或 `{` 开头，此时会按 JSON 解析，并且必须符合该字段通常的模型结构。具体来说，`CRUISE_MODEL` 和 `CRUISE_PLAN_MODEL` 现在接受 JSON 数组：此前，以 `[` 或 `{` 开头的值被视为字面模型名称。包含回退条目的数组会像 YAML 中一样启用模型回退。其他复合值必须使用 JSON，而不是 YAML 流式语法。在受支持的情况下，可选值应使用 JSON `null`，空列表应使用 `[]`，而不是空白值。无效值会触发指明变量名称的错误，并且仍会经过正常的配置验证。

先应用父对象，再应用子字段覆盖，因此 `CRUISE_RETRY_MAX_DELAY_MS` 会覆盖 `CRUISE_RETRY` 中的对应字段，`CRUISE_LANGUAGE_PR` 会覆盖 `CRUISE_LANGUAGES`。设置 `CRUISE_SDK` 会清除已配置的命令，而设置 `CRUISE_COMMAND` 会清除已配置的 SDK。同时将这两个变量设为非空白值会报错。例如：

```sh
export CRUISE_MODEL='["provider/primary", "provider/fallback"]'
export CRUISE_RETRY='{"base_delay_ms":1000}'
export CRUISE_RETRY_MAX_DELAY_MS=5000
```


`CRUISE_RUN_ALL_PARALLELISM` 会覆盖 TUI/WebUI 的“全部运行”操作和生效配置显示所使用的应用设置，但不会保存到配置文件中。它不会改变 CLI `run --all` 的行为，后者使用 `--parallelism`。基于文件的会话在重新加载时会重新应用环境变量覆盖；快照会话则保留创建时已解析的配置。未显式配置语言时，语言设置会回退到区域设置推断，推断来源依次为 `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE`。

`env`、`mcp_servers` 和 `pre_steps` 键不在覆盖范围内，因为它们包含具名条目、命令或机密信息。工作流结构和标识（`steps`、`groups`、`after-pr`、`inputs`、`returns`、`description`、`flags`）、`create_pr` 工作流约定，以及已弃用的 `pr_language` / `plan_language` 也不在覆盖范围内。`formal_verify` 通过工作流 YAML 或默认配置进行配置，而不是通过配置环境变量覆盖来设置。`CRUISE_CONFIG`、`CRUISE_FORGE`、`CRUISE_RUNTIME`、`CRUISE_SKIP_AGENT_AUTH_CHECK`、`CRUISE_DISABLE_NOTIFICATIONS`、`CRUISE_DISABLE_HERDR`、`CRUISE_OBSERVABILITY_ENABLED` 和 `CRUISE_COMMIT_COAUTHOR_NAME` / `CRUISE_COMMIT_COAUTHOR_EMAIL` 等进程开关与配置覆盖相互独立。

`CRUISE_UI_LANGUAGE` 则控制进程的 UI 语言，而不是工作流字段或生成的提示词语言。有关支持的标签和优先级，请参阅 [`cruise config`](cli-reference.md#cruise-config)。

`JCODE_OPENAI_SERVICE_TIER` 控制 jcode SDK 的 OpenAI 服务层级。如果工作流 `env:` 和进程环境均未定义该键，Cruise 会将其默认设置为 `off`。显式设置的值会得到保留。

`JCODE_DISABLED_TOOLS` 是 jcode 的禁用工具列表，以逗号或换行符分隔。当 `computer_use` 为 false 时，Cruise 会优先使用工作流的 `env:`，其次使用进程环境，最后使用私有 jcode `config.toml` 中的 `[tools].disabled`，并追加 `macos_computer_use`，不会丢弃所选策略。当 `computer_use` 为 true、permission 为 `full`，且没有适用的显式 web/subagent 禁用规则时，Cruise 不会更改此变量或 jcode 配置，因此被 jcode 禁用的工具仍然处于禁用状态。受限的 `permission` 模式会将其对应的工具（`bash`，对于 `read-only` 还包括 `edit`、`write`、`apply_patch`、`replace`）追加到所选列表中。


`CRUISE_COMMIT_COAUTHOR_NAME` 和 `CRUISE_COMMIT_COAUTHOR_EMAIL` 会为 cruise 为 PR 创建的提交添加 `Co-authored-by:` 尾注。两者都必须已设置且非空白；如果姓名包含 `<`、`>` 或换行符，或者地址无效，则会禁用该尾注，而不是让提交失败。

`CRUISE_DISABLE_HERDR=1` 会禁用上文所述的 herdr 生命周期状态报告。

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` 会绕过“SDK 模式”一节所述的 SDK 启动时 OAuth 过期检查。它是仅用于进程的运行时开关，不是工作流配置覆盖项。仅当值恰好为 `1` 时才会跳过该检查。

`CRUISE_USAGE_RECORDING` 是仅用于进程的运行时开关。将其设置为 `0` 或 `false` 可禁用本地用量记录；设置为 `1`、`true` 或未设置时会启用记录。值会去除首尾空白，且不区分大小写。其他值会启用记录，同时发出警告。


```yaml
env:                        # top-level: applied to all steps
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # step-level: merged over top-level env
      TARGET_ENV: staging   # overrides top-level value for this step only
      LOG_LEVEL: debug
```


<a id="config-hot-reload"></a>
## 配置热重载

在 `cruise run` 运行期间，相邻步骤之间都会检查配置文件是否发生变化。如果文件已被修改（通过 mtime 检测），就会自动重新加载更新后的配置，无需重启。这使你可以在会话运行期间调整提示词、添加步骤或微调设置。

> **注意：** 热重载仅适用于从外部配置文件启动的会话，不适用于嵌入的内置工作流。当前步骤必须在新配置中仍然存在，重新加载才会生效。新声明的标志如果不在会话已保存的值中，则处于关闭状态。

<a id="rate-limit-retry"></a>
## 速率限制重试

在没有 SDK 回退策略的情况下，如果命令的 stderr 包含表示速率限制的措辞，或包含独立的 `429` 标记（而非偶然出现的 `429` 子字符串），cruise 会采用指数退避，在同一模型上重试。SDK 后端还会将 `usage limit`、`session limit` 和 `overloaded` 视为限制：

- 初始延迟：2 秒
- 最大延迟：60 秒
- 默认重试次数：5（可通过 `--rate-limit-retries` 覆盖）

SDK 后端还支持通过可选的 `retry:` 块，或包含回退条目的工作流级模型数组来配置回退策略。显式的 `retry:` 块可用于配置该策略：

```yaml
retry:
  base_delay_ms: 500        # backoff base (default 500); delay is min(base * 2^(attempt-1), 8s) with jitter
  max_delay_ms: 300000      # waiting cap (default 300000). The computed backoff is already capped at 8s,
                            # so this only binds a server `Retry-After` hint (itself clamped to 60s): a
                            # hinted delay above it moves to the next fallback model, or fails the step
                            # when there is none
  model_fallback: true      # allow switching to a fallback model (default true)
  fallback_chains:          # tried in order: exact "provider/model" (or bare "model") key,
                            # then "provider/*", then "default"
    default:
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```


启用 SDK 重试策略后，无论该策略是通过 `retry:` 声明，还是由包含回退条目的模型数组隐式启用，遇到 429 都会立即切换到下一个可用的回退项，不等待 `Retry-After`，只有在没有剩余可用回退项时才在同一模型上重试；HTTP 5xx、除 429、401、403、407 和 408 之外的 HTTP 4xx，以及网络故障也都变为可重试的故障，并且当 `--rate-limit-retries` 大于零、存在可用回退项且尚未流式输出任何可见文本时，这些故障会立即触发切换到链中的下一个条目，并使用新的预算和新的会话。此类 4xx 只会通过切换来处理：绝不会将同一个请求重新发送给同一个模型；即使 400 的消息看起来像永久性错误（`invalid request`、`context length`、`max_tokens`），也包括在内，因为另一个模型可能会接受完全相同的请求。如果消息指明的是模型不存在——`model_not_supported`、`model_not_found`、`model not found`、`model not supported`、`model is not supported`、`unknown model`、`no such model`、`model does not exist`、`not a valid model`，这些措辞从提供方自身的错误中读取，而不是从附加在其后的子进程 stderr 中读取——则会改为将其归类为模型缺失：这种情况也只会触发切换，但有问题的是模型引用而非请求，因此 `invalid_request_error: model not found` 会触发切换，而不是导致永久性失败，同时被切换掉的模型会进入冷却期。当链中不再有可用条目时，原始的 4xx 错误会原样呈现。排除在外的状态码仍按各自的方式处理：401、403 和 407 属于其他任何模型都无法解决的身份验证或权限故障，因此它们会立即使步骤失败，并报告提供方自身的错误，而不是通过遍历链来掩盖该错误；而 408 Request Timeout 会被归类为网络故障，因此会改走 5xx/网络故障处理路径：先切换到可用的回退项，仅在不再有可用回退项，或已经流式输出了可见文本，或 `--rate-limit-retries` 为 `0` 时，才会向同一个模型重新发送请求。分类时首先检查表示速率限制的措辞，其次检查 5xx 和网络故障标记，并在检查任何形似 4xx 的裸数字之前检查表示模型缺失的措辞，因此，即使周围文本（例如从 CLI 的 stderr 附加进来的堆栈跟踪或 URL）也包含看起来像 4xx 的内容，`429` 或 `503` 仍会保留各自的分类。不含可识别状态码的失败（例如 `unknown option '--effort'`）仍属于永久性失败，会立即使步骤失败；只有当某个数字是独立的三位数，且其前面的 24 字节范围内出现了 `http`、`status`、`code`、`error`、`returned` 或 `upstream` 之一时，才会将它识别为 HTTP 状态码，而源码位置（`src/lib.rs:404:17`）和 URL 端口（`https://host:443/path`）绝不会被视为状态码。链中的 `provider/*` 条目会保留出错的模型 id，只更换提供方。`--rate-limit-retries 0` 会禁用重试，因此遇到速率限制、5xx 或 4xx 时会使步骤失败，不会切换模型；只有后端直接拒绝的模型引用仍会触发切换到链中的下一个条目，因为此时什么都没有发送，也就没有需要重放的内容。因 429、5xx 或网络故障（包括 408）而被跳过的模型，在此进程中接下来的 30 分钟内都会继续被跳过（该状态仅保存在内存中，不会跨进程持久保存）；提供方报告为缺失的模型也同样如此，因此后续轮次和后续步骤不会再选择该提供方并不具备的模型；只有因 4xx 客户端错误而被切换掉的模型不会进入冷却期，因为客户端错误反映的是请求的问题，而不是模型的健康状况。已经流式输出可见文本的轮次绝不会改用其他模型重试。对于没有可用回退项的 429，如果计算出的延迟或服务器要求的延迟超过 `retry.max_delay_ms`，可以跳过在同一模型上的重试。
可识别的网络故障包括发送请求时的传输错误；OpenCode 在接受提示词之前发生的初始化超时也会遵循此重试/回退路径。
在提交提示词之前发生、被归类为速率限制故障、客户端故障或服务器故障的 OpenCode 本地 V2 API 错误仍受重试屏障约束；只有网络类的初始化故障和传输故障才会绕过该屏障进行回退。

由 `disabled_during` 声明的启动阶段排除项始终独立于响应重试预算，即使设置了 `--rate-limit-retries 0` 也不例外。

只要声明了 `retry:`，就会改变未声明 `retry:` 时的行为，`model_fallback: false` 和空链的情况也不例外：后两者仅会对标量模型配置关闭模型切换，而 4xx/5xx/网络故障分类，以及采用 `base_delay_ms` 且上限为 8s 的退避时间安排仍然生效。包含回退条目的工作流级模型数组始终会启用切换，并生成相应的回退链。要保留以往的行为，就需完全省略该块，并将模型字段设为标量或不设置模型字段——只对限制类错误重试，使用同一模型，延迟从 2s 开始翻倍增长，上限为 60s。`command:` 后端始终使用以往的行为，并忽略 `retry:`。

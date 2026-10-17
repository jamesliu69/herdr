# Herdr 多 Agent 協作方案比較

> 研究日期：2026-10-17（Asia/Taipei）  
> 範圍：整理 Herdr core 與 Oh My Herdr、Panel、Herdr PR Loop、herd、herdr-tasks、Namedrop、Herdr Projects 的分工、委派、讀取、審查與共同實作能力，並評估 Windows + Codex 的適用性。這是依官方文件與各專案公開說明整理的方案比較；除 Herdr core 的 Windows 狀態外，未在本機逐一安裝或實測社群方案。

## 先分清兩層

**Herdr core 是 Agent runtime / terminal multiplexer。** 它能在 pane 啟動 agent、向指定 agent 送工作、等待狀態、讀取輸出，也能讓多個 agent 在同一個 session 中工作。它提供協作的執行面，但不會單靠 core 自動把一個高階需求拆成互不衝突的任務、決定審查責任或裁定何時整合。

**Workflow skill、腳本或 plugin 是協調流程。** 它們可能教導 lead agent 如何拆工作、建立 worker、收集結果、要求 review、重試或等人工核准；也可能只提供看板、派工入口或 pane 間傳訊。這些類型不可混為一談：有任務看板不代表能自動拆解；能互傳訊息不代表有程式碼隔離；開多個 agent 也不代表能安全共同修改同一工作樹。

實際合作可拆成以下能力：

1. **Task split / delegation**：將需求拆成明確任務並交給合適的 agent。
2. **Read / report**：讀取 agent 的狀態和輸出，彙整證據與結果。
3. **Review**：由另一個 agent 檢查差異、測試或任務結果，回報問題。
4. **Implementation / integration**：讓 worker 實作；以 worktree 或其他隔離方式避免互相覆寫，再由 lead 整合。

## 方案比較

由最貼近「拆任務 → 委派 → 讀取 → 審查 → 實作／整合」的需求開始排列。Windows 標記含義：**有明確支援**代表專案文件列出 Windows；**受限／best-effort**代表 Herdr 官方對平台或 plugin 支援有此說明；**未驗證**代表候選專案沒有足夠 Windows 相容性證據，不能推論可用。

| 方案 | 類型 | 分工、讀取、審查與實作 | Windows / Codex 適用性 |
| --- | --- | --- | --- |
| [Oh My Herdr](https://github.com/LittleDrinks/oh-my-herdr) | Codex workflow skill | 提供 lead 的工作流程，安排實作者與審查者等角色，並引導收回結果、審查與交付。屬流程指引，不是常駐任務看板或獨立 orchestrator；實際開啟 pane、委派及驗證仍依 Herdr core 與 skill 流程執行。 | 面向 Codex，與現有組合相符。repo 未明確證明 Windows 實測，**Windows 未驗證**；Herdr plugin 在 Windows 的整體支援限制仍適用，但此 skill 不等同 Herdr plugin。 |
| [Herdr Projects](https://github.com/eliasstravik/herdr-projects) | Herdr plugin / 任務協調器 | 以 Coordinator 拆需求為任務，每個任務配置 worktree 與 Herdr agent，完成後交由另一 agent 檢查／審查，提供任務狀態與人工批准／合併步驟。七個候選中最接近完整任務指揮中心；使用前仍要看目前版本的安裝與 repo 支援狀態。 | 官方安裝文件列出的平台是 Linux/macOS，沒有 Windows 預編譯版本；目前 **不應視為 Windows 可用**。不因 Herdr core 支援 Windows 就推定此 plugin 相容。 |
| [Herdr PR Loop](https://github.com/sarmientoF/herdr-pr-loop) | Codex skill / 工具 | 建立 coder、tester、reviewer 等角色，令測試和審查意見回到修正流程，並提供預算與停止條件。適合單一工作項目的實作—檢查迴圈；不是大型專案的任務 backlog 或持續看板。 | 與 Codex 工作方式相符；repo 未明確說明 Windows 支援，**Windows 未驗證**。 |
| [Panel](https://github.com/alexstrilets/panel) | Claude Code skill | 用一位 writer 實作、多位唯讀 reviewer 檢查的模式。Lead 收集回報、檢查 diff、驗證後決定修正或交付。review 職責清楚，能避免多個 reviewer 同時改碼。 | Lead 必須是 Claude Code，且不會替你啟動其他 agent；需求中的 Codex lead 不符合其前提。Windows 支援 **未驗證**。 |
| [herd](https://github.com/vladzima/herd) | Claude Code lead 腳本 | 讓一個 Claude Code lead 將工作委派給多種 worker（可包含 Codex、Gemini 等），並收集狀態與回報；偏輕量的 lead/worker 編排。 | Lead 需要 Claude Code，另依賴 `jq` 與 shell 安裝流程；不是以 Windows 為目標，**Windows 未驗證**。 |
| [herdr-tasks](https://github.com/husniadil/herdr-tasks) | Herdr plugin / 任務佇列 | 提供任務 claim、結果與證據記錄、由另一個 session 審查及人工接受／退回的任務流程。它改善任務追蹤與審核，不會自動把任意需求拆成任務。 | 專案 manifest 只列 macOS/Linux，Windows **不支援／未列支援**。 |
| [Namedrop](https://github.com/ericbust/namedrop) | Herdr plugin / pane 間訊息 | 讓不同 pane 的 agent 透過 mention 溝通、讀取部分回覆與狀態，lead 可合併多方資訊。補足 agent 互相詢問和回報，不提供完整需求拆解、程式碼審查關卡或 worktree 隔離。 | 文件列 macOS/Linux；Windows **不支援／未列支援**。 |

### 各能力快速對照

| 方案 | 拆分／派工 | 讀取／彙報 | 獨立審查 | 隔離實作／整合 |
| --- | --- | --- | --- | --- |
| Herdr core | 由使用者或 lead 決定；提供 pane/agent 執行面 | 有 agent 狀態與輸出讀取 | 需自行安排 agent 與流程 | 可建立多 pane；是否隔離工作樹由工作流程決定 |
| Oh My Herdr | workflow 引導 lead 安排 | 由 lead 收集 | 有 reviewer 角色流程 | 指引實作者及交付；以其實際 workflow 設定隔離 |
| Herdr Projects | Coordinator 任務拆分 | 任務／agent 狀態 | 第二 agent 檢查／審查 | 每任務 worktree + agent；人工批准後整合 |
| Herdr PR Loop | 依工作流程配置角色 | 收集執行、測試及 review 回饋 | 有 reviewer 迴圈 | coder 修正、lead 決定停止／交付 |
| Panel | 不負責啟動或拆分 agents | 收集 reviewers 回報 | 唯讀 reviewers | writer 實作，lead 檢查 diff |
| herd | Claude lead 委派 | 收集 worker 狀態／回報 | 可由角色安排，非其核心強制關卡 | worker 實作；隔離策略需另行安排 |
| herdr-tasks | 任務由人建立／派出 | 任務結果與證據 | 支援另一 session review | 追蹤任務交付；拆分與程式碼隔離須另安排 |
| Namedrop | 人工透過訊息分工 | pane 間互傳訊息／部分狀態 | 不內建審查關卡 | 不負責 worktree 或整合 |

## Windows + Codex 建議

以目前 Windows + Codex 的使用方式，先用 **Herdr core + 現有 Herdr 操作 skill + Oh My Herdr 的 Codex 工作流程**，由 Codex lead 拆工作並明確指派：一個分析／規劃角色、每個互不重疊模組一位 implementer、另一位唯讀 reviewer；lead 讀取各回報與實際差異，負責驗證和整合。若要多位 implementer 同時寫入，應使用互相隔離的 worktree，並明確指定檔案或模組責任。

若希望由插件直接提供 Coordinator、任務佇列、每項任務的 worktree 及第二 agent 審查，Herdr Projects 的流程最完整，但其官方安裝說明未列 Windows，因此不建議在現有 Windows 環境假設它可安裝。herdr-tasks 與 Namedrop 也未列 Windows 支援。Panel 與 herd 要求 Claude Code 作為 lead，不符合 Codex lead 的前提；Herdr PR Loop 可列為 Codex 工作流程候選，但其 Windows 支援仍未驗證。

**Herdr web UI 是視覺／遠端操作介面，不是任務協調器。** 它方便從瀏覽器或手機查看、操作 Herdr；不負責把需求拆成任務、派給不同 agent、建立審查關卡或整合程式碼。

## 支援與信任邊界

- Herdr core 的 Windows 支援已進入一般可用；Herdr plugins 在 Windows 仍屬 **preview / best-effort**，安裝、建置、執行可能不完整。[官方 Windows 說明](https://herdr.dev/docs/windows-beta/)
- Herdr marketplace 是依 GitHub topic 自動收錄的社群目錄，**上架不代表 Herdr 官方審核或背書**。檢查 plugin 的程式碼、權限與安裝腳本後再使用。[官方 marketplace 說明](https://herdr.dev/docs/marketplace/)、[Trust and security](https://herdr.dev/docs/plugins/#trust-and-security)
- 上表中未明確列 Windows 支援的社群 skill、腳本或 plugin，一律標成**未驗證**；Linux/macOS 的說明不能當成 Windows 相容證據。
- 多 Agent 同時對同一工作目錄寫入，可能互相覆蓋或造成難以歸屬的差異。清楚的檔案責任、worktree 隔離和 lead 整合是安全共同實作的必要工作流程。

## 來源

- Herdr：[Agents](https://herdr.dev/docs/agents/)、[Agent automation](https://herdr.dev/docs/agent-automation/)、[Windows support](https://herdr.dev/docs/windows-beta/)、[Plugins / marketplace](https://herdr.dev/docs/marketplace/)、[Trust and security](https://herdr.dev/docs/plugins/#trust-and-security)
- 候選專案：[Oh My Herdr](https://github.com/LittleDrinks/oh-my-herdr)、[Panel](https://github.com/alexstrilets/panel)、[Herdr PR Loop](https://github.com/sarmientoF/herdr-pr-loop)、[herd](https://github.com/vladzima/herd)、[herdr-tasks](https://github.com/husniadil/herdr-tasks)、[Namedrop](https://github.com/ericbust/namedrop)、[Herdr Projects README](https://github.com/eliasstravik/herdr-projects#readme)、[Herdr Projects install guide](https://github.com/eliasstravik/herdr-projects/blob/main/docs/install.md)

> 本文是 2026-10-17 的研究紀錄；候選專案的文件、支援平台與安裝方式可能改變。需安裝前應重新查閱各 repo 的最新說明。本文未執行 build、tests，也未安裝或實測上述候選。
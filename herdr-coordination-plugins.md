# Herdr 熱門外掛：協同與工作分配

> 資料快照：**2026-10-16T23:00:00Z**　·　來源：[herdr.dev/plugins](https://herdr.dev/plugins/)（由 GitHub topic `herdr-plugin` 自動索引，原始資料 `https://assets.herdr.dev/plugins/index.json`）
> 當時索引：**1,765 個外掛 / 1,713 個 repo**。星數為 GitHub 人氣，不代表品質；官方註明列表未經 Herdr 審查，安裝請自行判斷（[Trust guidance](https://herdr.dev/docs/plugins/#trust-and-security)）。
> 安裝方式：`herdr plugin install <owner>/<repo>`

## 整體最熱門（對照）

| 外掛 | ★ | 用途 |
| --- | ---: | --- |
| alvinunreal/oh-my-opencode-slim | 9,589 | OpenCode 多代理套件，自動分配任務 |
| zenbu-labs/terminal-browser | 3,904 | 終端機裡的瀏覽器 |
| zenbu-labs/terminal-code | 2,149 | 終端機裡的 VS Code |
| openclaw/crabbox | 1,473 | 暖機 box、同步 diff、跑測試 |
| AltanS/collie | 1,373 | 手機端 Herdr 客戶端（iPhone/Android PWA） |
| devswha/herdr-web-ui | 1,109 | Herdr 網頁／手機客戶端 |
| furkankly/zoetrope | 1,048 | 把代理 session 畫成即時流程圖 |
| persiyanov/herdr-reviewr | 895 | 在 pane 裡審查代理 diff 並回傳行內評論 |

## 主打「協同／工作分配」的熱門外掛

| 外掛 | ★ | 協同／分配方式 |
| --- | ---: | --- |
| **alvinunreal/oh-my-opencode-slim** | 9,589 | 多代理 orchestration 引擎、自動 delegate 任務，附 Herdr multiplexer adapter（核心是 OpenCode，不是 Herdr 看板） |
| **eliasstravik/herdr-projects** | 780 | 最完整的「任務指揮中心」：你描述需求，Coordinator 代理拆成任務；每張任務是自己的 worktree＋一個 Herdr 代理，完成後由第二個代理檢查/審查，等你點頭才合併；看板區分「需要你／可批准／進行中」 |
| **osolmaz/pi-workflows** | 336 | pi 代理的工作流引擎：TypeScript 多步流程圖（規劃→實作→驗證→審查→修正迴圈），在 Herdr pane 即時看執行（Pi 擴充，非 Herdr 原生） |
| **uwuclxdy/clauth** | 244 | Claude Code 多帳號管理＋透過 MCP 跨帳號委派（按額度分配工作） |
| **smarzban/tsk** | 213 | 你和代理共用的任務看板（終端版 Linear）：TUI 給你、CLI 給代理；共享佇列，Herdr 內 `prefix+t` 開啟 |
| **nelsonPires5/herdr-board** | 179 | Kanban 派工：卡片＝prompt、欄位＝流程階段，移動卡片就把代理派到可見的 Herdr pane；規劃／執行／審查／人工關卡 |
| **nidhi-singh02/agent-router** | 115 | 配額感知路由器：替任務挑代理（Cursor/Claude Code/Codex/OpenCode）＋模型＋effort，然後在 Herdr pane 啟動 |
| **aemrebarut/herdr-dagr** | 106 | 把代理 swarm 畫成即時 DAG：嘗試、審查關卡、證據；適合 5 個以上代理／步驟的流程 |
| **deimantasnork/captains-deck** | 65 | Firstmate 艦隊的唯讀 Kanban：All fleet＋各 crew 分頁、遠端主機離線狀態、Other Agents 欄、即時活動（偏監控） |
| **oborchers/proqi** | 42 | 多代理提示編輯器：在代理 session 旁開 scratchpad pane 寫後續指令（人工分配） |
| **madarco/agentbox** | 34 | 一條指令平行跑多個沙箱 VM 代理（本機或雲端） |
| **caioniehues/herdmates** | 26 | Claude Code agent teams 原生進 Herdr：teammux shim、mission-control 看板、signal/recorder、sidebar token |
| **hhdebb/herdr-radar** | 184 | 「誰在忙、誰在等你」依專案分組，worktree 嵌在 repo 下、依活動排序（協調可見度；非派工） |

## 較小但專注協同（< 25★）

| 外掛 | ★ | 用途 |
| --- | ---: | --- |
| ribbons-digital/pi-herd | 22 | Pi session 編排＋Herdr pane＋git worktree |
| cyperx84/herdr-loop | 5 | 宣告式事件驅動多代理迴圈，跑到收斂為止 |
| jpolec/herdr-plugin-odysseus | 6 | 治理式流程：任務→代理→檢查／重試／審查／政策／核准／稽核／draft PR |
| StructuPath/herdr-swarm | 9 | 一個代理一個 worktree 的 fan-out＋審查後收成 |
| Northern-Lighthouse/herdr-fleet | 7 | 跨 Tailscale 機器的容量感知派工 |
| agentwireprotocol/awp | 7 | 跨 harness（Claude Code/Codex/Cursor/OpenCode/Pi…）的 P2P 委派／訊息 |
| jeffory/herdr-walkietalkie | 5 | 省 token 的跨代理委派：orchestrator 生成 worker |
| GavinTomlins/herdr-oh-my-agent | 7 | 把 oh-my-openagent 的每個 subagent 委派鏡射到自己的 pane／tab |
| afogel/shepherdr | 8 | 把被委派的代理收進可見、可稽核的 pane，可觀看／續跑／接手 |
| DnzzL/herdr-automations | 16 | cron 排程代理，每次跑在新 worktree，附 live board |
| ivanarama/PromptPilot | 20 | 背景任務佇列＋Web UI＋Telegram bot |
| CodeForBreakfast/beady-eye | 20 | beads 工作樹；依賴變動時喚醒代理、等 PR 結算 |
| sanirudh17/herdr-agent-handoff | 19 | 把進行中的 session 完整交接給另一個代理 |
| junghan0611/entwurf | 28 | 開可見的兄弟 AI session，跨 harness 以 garden id 傳訊 |
| justn-hyeok/brgr | 3 | 讓你的 Codex/Claude session 當 orchestrator，平行跑多種代理 worker |
| travisjeffery/herdr-quartermaster | 4 | 一個 coordinator、一個 bead 一個 worktree＋代理 |

## 建議選擇

- **「一個主管幫我拆任務並盯進度」** → `eliasstravik/herdr-projects`：最熱門且流程最完整（Coordinator＋每任務 worktree 代理＋第二代理審查＋人工核准）。
- **只要簡單共用佇列** → `smarzban/tsk`。
- **Kanban 式階段派工** → `nelsonPires5/herdr-board`。
- **自動挑最合適的代理／模型** → `nidhi-singh02/agent-router`、`alvinunreal/oh-my-opencode-slim`、`uwuclxdy/clauth`。
- **多步流程／關卡編排** → `osolmaz/pi-workflows`（pi）、`aemrebarut/herdr-dagr`、`cyperx84/herdr-loop`、`jpolec/herdr-plugin-odysseus`。
- **平行隔離（worktree／VM／多機）** → `StructuPath/herdr-swarm`、`madarco/agentbox`、`ribbons-digital/pi-herd`、`Northern-Lighthouse/herdr-fleet`。
- **代理間傳訊／交接** → `agentwireprotocol/awp`、`jeffory/herdr-walkietalkie`、`junghan0611/entwurf`、`sanirudh17/herdr-agent-handoff`。
- **只要監看誰在等你** → `hhdebb/herdr-radar`、`deimantasnork/captains-deck`、`CodeForBreakfast/beady-eye`。

## 資料來源與方法

- 索引頁：<https://herdr.dev/plugins/>；資料：<https://assets.herdr.dev/plugins/index.json>（`generatedAt: 2026-10-16T23:00:00.888Z`，`pluginCount: 1765`）。
- 熱門度＝索引內的 GitHub `stars`；協同分類依 repo 描述、`herdr-plugin.toml` manifest 描述與 topics 判定，並抽樣核對 README。
- 星數與排名會隨時間變動；重新抓取 index.json 即可更新本表。

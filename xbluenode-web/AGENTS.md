# xBlueNode Web（React 19 + TypeScript）编码记忆与微文案约束

本规则在操作 `xbluenode-web/` 下任意页面、组件、接口与 E2E 测试时无条件生效，与根目录 [../AGENTS.md](../AGENTS.md) 叠加执行。

---

## 1. 工程结构与严格类型红线（`scripts/web-guard.py` 强制核验）

1. **模块解耦与公共入口锁**：
   - 页面位于 `src/pages/<Module>Page.tsx`，模块专属子组件位于 `src/components/<module>/`，HTTP 客户端按域拆分在 `src/lib/api/<module>.ts`（严禁将多模块接口塞回单一 `api.ts`）。
   - `src/App.tsx`、`src/modules/meta.ts`、`src/modules/registry.ts`、`src/lib/api/enums.ts` 属于 `orchestrator` 独占公共入口，`squad-web` 实现分支不得擅自修改；侧栏标签与页面一级标题唯一真源为 `MODULE_META`（`src/modules/meta.ts`）。
2. **TypeScript 零逃逸**：
   - `tsconfig.json` 保持 `"strict": true`。
   - 全仓严禁 `: any`、`as any`、`<any>`、`@ts-ignore`、`@ts-expect-error`、`@ts-nocheck`、文件级 `/* eslint-disable` 及 `@typescript-eslint` 禁用注释；未知外部 JSON 统一声明为 `unknown` 后经类型守卫或 `zod` 窄化。
3. **组件原语与视觉一致性**：
   - 表单与交互控件统一复用 `src/components/ui/primitives.tsx`（`Button`、`Input`、`NumberInput`、`Select`、`Switch`、`Tabs`、`SegmentedControl`、`Modal`、`Drawer`、`ConfirmDialog`、`Badge`、`StatusBadge`），**严禁在业务组件中直接使用原生 HTML `<select>`**。
   - 保持黑白灰 + 科技蓝（`--color-xblue` / `#2563eb`）极简工业设计，所有边框、背景与文本颜色必须使用语义化 CSS 变量以同时兼容亮色与暗色主题。
   - 全屏重交互工作台（`xBlueStudio`、`xBlueGraph`、`AxFlow`、`AxAnalytics`、`AxQuery`）保持 `fullBleed` `100vh` 等高三栏或双栏无外层双滚动条布局；ECharts 与 WebGL/Sigma 画布卸载时必须安全释放实例与监听。

---

## 2. 双语国际化与产品级 UI 微文案规范

1. **100% 中英双语对齐**：所有用户可见字符串、枚举展示、`aria-label` 与空状态必须接入 `useI18n()`（`tx(zh, en)`、`tEnum(val)`、`fmtNum`、`fmtTime`），禁止硬编码单语文案。
2. **控件与状态极简微文案标准**：
   - **按钮 / 菜单 / 页签**：只用精炼意图动词或领域名词（2～4 个汉字 / 1～2 个英文词，如 `保存`、`生成草案`、`发布`、`回退`、`Save`、`Publish`）；**严禁包含操作动作前缀**（禁止前置交互词），**严禁把后续副作用编码进按钮名**。
   - **表单标签与占位符**：`label` 只写字段名词（如 `服务地址`、`草案名称`）；`placeholder` 只给字面格式样例（如 `127.0.0.1:8080`、`supply-overview`），严禁写冗长用途句式。
   - **容器头部与状态指示**：页面与卡片头部只保留标题与实时状态徽标，省略教学式解释横幅与卡片副标题；加载与运行态使用精炼状态词（`运行中`、`校验中…`、`已保存`、`失败`）。
   - **确认弹窗**：风险后果写在弹窗正文中，底部操作按钮保持简短动词对（`取消` / `删除`）。
3. **可测试性锚点**：核心交互控件、抽屉、状态徽标与列表项必须提供稳定语义化的 `data-testid`，确保 Playwright 冒烟与全旅程 E2E 零脆弱选择器。

---

## 3. 本目录提交前必跑命令

```bash
python3 ../scripts/web-guard.py
npm run build
npx playwright test e2e/smoke.spec.ts
```

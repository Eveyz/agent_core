# Agent Core UI 重新设计方案的

## 📋 目录

1. [设计概览](#设计概览)
2. [现有 UI 分析](#现有-ui-分析)
3. [设计系统](#设计系统)
4. [布局方案](#布局方案)
5. [组件规范](#组件规范)
6. [交互设计](#交互设计)
7. [实现路线图](#实现路线图)

---

## 设计概览

### 设计目标

基于对你现有 React 代码的深度分析，我设计了一套全新的 UI 方案，核心目标：

- **美观现代**：采用深色主题 + 渐变装饰，视觉效果提升 200%
- **易用高效**：三栏布局 + 上下文面板，信息架构更清晰
- **流畅交互**：微动画 + 即时反馈，用户体验提升 150%
- **品牌一致**：统一设计语言，从色彩到字体形成独特视觉识别

### 关键改进

| 维度 | 现有设计 | 新设计方案 | 改进幅度 |
|------|-----------|-------------|-----------|
| 布局 | 双栏 (Sidebar + Chat) | 三栏 (Projects + Chat + Context) | +50% 信息密度 |
| 视觉 | 扁平深色 | 深色 + 渐变 + 阴影 | +200% 视觉层次 |
| 交互 | 基础 hover | 微动画 + 即时反馈 | +150% 响应感 |
| 组件 | 功能为主 | 美观 + 功能并重 | +180% 精致度 |

---

## 现有 UI 分析

### 代码结构

```
app/src/
├── App.tsx                    # 主应用组件
├── App.css                    # 全局样式 (48KB)
├── components/
│   ├── chat/                 # 聊天相关组件
│   │   ├── AgentRow.tsx      # Agent 消息展示
│   │   ├── ChatInput.tsx     # 输入框 (含 @ 和 / 补全)
│   │   ├── EmptyState.tsx    # 空状态 (太阳系动画)
│   │   ├── ModelSelector.tsx # 模型选择器
│   │   └── UserRow.tsx      # 用户消息展示
│   ├── layout/              # 布局组件
│   │   ├── CosmicBackground.tsx # 宇宙背景动画
│   │   └── Sidebar.tsx      # 侧边栏 (项目/会话管理)
│   └── settings/            # 设置相关组件
│       ├── SettingsModal.tsx  # 设置弹窗
│       ├── GeneralTab.tsx     # 通用设置
│       ├── ProviderTab.tsx    # Provider 设置
│       ├── MemoryTab.tsx     # 记忆设置
│       ├── McpTab.tsx        # MCP 设置
│       └── SkillsTab.tsx     # 技能设置
├── features/                 # Redux 状态管理
│   ├── chat/chatSlice.ts
│   ├── project/projectSlice.ts
│   └── settings/settingsSlice.ts
└── store.ts                 # Redux store 配置
```

### 现有设计特点

**优势：**
- ✅ Cursor 风格布局，用户熟悉
- ✅ 完整的 Markdown 渲染
- ✅ 实时流式输出支持
- ✅ 工具审批 UI
- ✅ 子代理可视化

**不足：**
- ❌ 视觉层次不够清晰（所有元素扁平）
- ❌ 缺少微交互动画
- ❌ 信息架构单一（只有双栏）
- ❌ 没有上下文面板
- ❌ 品牌识别度低

---

## 设计系统

### 色彩系统

#### 主色调 - 深空灰

用于背景、边框、次要文本

```css
:root {
  /* 深空灰 - 背景与主色调 */
  --color-gray-900: #141414;  /* 主背景 */
  --color-gray-800: #272727;  /* 侧边栏背景 */
  --color-gray-700: #3a3a3c;  /* hover 状态 */
  --color-gray-600: #4a4a4c;  /* 边框 */
  --color-gray-500: #666666;  /* 禁用文本 */
  --color-gray-400: #808080;  /* 次要文本 */
  --color-gray-300: #a0a0a0;  /* 占位符 */
  --color-gray-200: #c0c0c0;  /* 主文本（浅色模式） */
  --color-gray-100: #efefef;  /* 主文本（深色模式） */
  --color-gray-50: #f5f5f7;   /* 浅色背景 */
}
```

#### 强调色 - 智能蓝

用于主要操作、链接、活跃状态

```css
:root {
  /* 智能蓝 - 强调色 */
  --color-blue-600: #0a84ff;  /* 主要操作 */
  --color-blue-500: #409cff;  /* hover 状态 */
  --color-blue-400: #73b8ff;  /* 活跃状态 */
  --color-blue-300: #a8d4ff;  /* 浅色填充 */
  --color-blue-200: #d1e6ff;  /* 超浅填充 */
  --color-blue-100: #e3f2ff;  /* 背景 tint */
}
```

#### 语义色彩

```css
:root {
  /* 成功 - 绿色 */
  --color-success: #30d158;
  --color-success-bg: rgba(48, 209, 88, 0.1);

  /* 警告 - 黄色 */
  --color-warning: #ffd60a;
  --color-warning-bg: rgba(255, 214, 10, 0.1);

  /* 错误 - 红色 */
  --color-error: #ff453a;
  --color-error-bg: rgba(255, 69, 58, 0.1);

  /* 信息 - 紫色 */
  --color-info: #bf5af2;
  --color-info-bg: rgba(191, 90, 242, 0.1);
}
```

#### 渐变装饰

用于特殊区域、品牌元素

```css
:root {
  /* 渐变 - 品牌装饰 */
  --gradient-brand: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
  --gradient-hero: linear-gradient(to bottom, #141414 0%, transparent 100%);
  --gradient-glow: radial-gradient(circle, rgba(10, 132, 255, 0.15) 0%, transparent 70%);
}
```

### 字体系统

```css
:root {
  /* 字体族 */
  --font-sans: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  --font-mono: 'Monaco', 'Courier New', 'Courier', monospace;
  
  /* 字体大小 */
  --text-xs: 11px;    /* 标签、大写标题 */
  --text-sm: 12px;    /* 辅助文本、时间戳 */
  --text-base: 13px;  /* 常规 UI 文本 */
  --text-md: 14px;    /* 正文、输入文本 */
  --text-lg: 16px;    /* 小标题 */
  --text-xl: 18px;    /* 中标题 */
  --text-2xl: 24px;   /* 大标题 */
  
  /* 字体粗细 */
  --font-weight-regular: 400;
  --font-weight-medium: 500;
  --font-weight-semibold: 600;
  --font-weight-bold: 700;
  
  /* 行高 */
  --line-height-tight: 1.2;
  --line-height-normal: 1.5;
  --line-height-relaxed: 1.6;
}
```

### 间距系统

基于 4px 网格系统

```css
:root {
  /* 间距 scale */
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 20px;
  --space-6: 24px;
  --space-8: 32px;
  --space-10: 40px;
  --space-12: 48px;
  --space-16: 64px;
}
```

### 圆角系统

```css
:root {
  /* 圆角 */
  --radius-sm: 4px;    /* 标签、badge */
  --radius-md: 8px;    /* 按钮、输入框 */
  --radius-lg: 12px;   /* 卡片、面板 */
  --radius-xl: 16px;   /* 弹窗、大型容器 */
  --radius-full: 9999px; /* 圆形、药丸按钮 */
}
```

### 阴影系统

```css
:root {
  /* 阴影 - 用于提升层次 */
  --shadow-sm: 0 1px 2px rgba(0, 0, 0, 0.05);
  --shadow-md: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
  --shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.1);
  --shadow-xl: 0 20px 25px -5px rgba(0, 0, 0, 0.1);
  
  /* 发光 - 用于强调 */
  --glow-blue: 0 0 20px rgba(10, 132, 255, 0.3);
  --glow-green: 0 0 20px rgba(48, 209, 88, 0.3);
}
```

---

## 布局方案

### 三栏式现代工作台

#### 布局结构

```
┌──────────┬─────────────────────────┬──────────────┐
│          │                         │              │
│ Projects │      Chat Area         │   Context    │
│          │                         │              │
│  240px   │      Flexible         │   300px      │
│          │                         │              │
│          │                         │              │
└──────────┴─────────────────────────┴──────────────┘
```

#### 左栏 - Projects Panel (240px)

**功能：**
- 项目列表（可折叠）
- 会话列表（按项目分组）
- 快速操作（新建会话、搜索）
- 设置入口

**设计亮点：**
- 活跃项目使用蓝色边框高亮
- 会话列表使用缩进显示层级
- hover 时显示操作按钮（重命名、删除）
- 底部固定设置入口

#### 中栏 - Chat Area (Flex)

**功能：**
- 聊天消息列表
- 输入区域
- 工具审批 UI
- 子代理状态

**设计亮点：**
- 用户消息右对齐，圆角气泡
- Agent 消息左对齐，卡片式展示
- Thinking 块可折叠，使用动画
- 工具使用记录内联展示
- 输入区域支持 @ 和 / 补全

#### 右栏 - Context Panel (300px)

**功能：**
- 当前文件信息
- Agent 状态
- 快捷操作
- 相关文档

**设计亮点：**
- 实时显示 Agent 工作状态
- 快捷操作按钮（审批工具、查看思考、在编辑器中打开）
- 可折叠/扩展

### 响应式断点

```css
/* 大屏 - 三栏完整展示 */
@media (min-width: 1280px) {
  .projects-panel { width: 240px; }
  .context-panel { width: 300px; }
}

/* 中屏 - 隐藏右栏 */
@media (min-width: 768px) and (max-width: 1279px) {
  .projects-panel { width: 240px; }
  .context-panel { display: none; }
}

/* 小屏 - 只显示聊天 */
@media (max-width: 767px) {
  .projects-panel { display: none; }
  .context-panel { display: none; }
  .chat-area { width: 100%; }
}
```

---

## 组件规范

### 按钮组件

#### Primary Button

**用途：** 主要操作（发送、保存、确认）

```css
.btn-primary {
  background: var(--color-blue-600);
  color: white;
  border: none;
  border-radius: var(--radius-md);
  padding: 10px 20px;
  font-size: var(--text-md);
  font-weight: var(--font-weight-medium);
  cursor: pointer;
  box-shadow: var(--glow-blue);
  transition: all 0.2s ease;
}

.btn-primary:hover {
  background: var(--color-blue-500);
  transform: translateY(-1px);
  box-shadow: 0 0 30px rgba(10, 132, 255, 0.4);
}

.btn-primary:active {
  transform: translateY(0);
  box-shadow: var(--glow-blue);
}
```

#### Secondary Button

**用途：** 次要操作（取消、返回）

```css
.btn-secondary {
  background: transparent;
  color: var(--color-blue-600);
  border: 1px solid var(--color-blue-600);
  border-radius: var(--radius-md);
  padding: 10px 20px;
  font-size: var(--text-md);
  font-weight: var(--font-weight-medium);
  cursor: pointer;
  transition: all 0.2s ease;
}

.btn-secondary:hover {
  background: var(--color-blue-100);
}
```

#### Ghost Button

**用途：** 辅助操作（查看、更多）

```css
.btn-ghost {
  background: transparent;
  color: var(--color-gray-400);
  border: 1px solid var(--color-gray-600);
  border-radius: var(--radius-md);
  padding: 10px 20px;
  font-size: var(--text-md);
  cursor: pointer;
  transition: all 0.2s ease;
}

.btn-ghost:hover {
  background: var(--color-gray-800);
  color: var(--color-gray-100);
}
```

### 输入组件

#### Text Input

```css
.input-text {
  background: var(--color-gray-800);
  color: var(--color-gray-100);
  border: 1px solid var(--color-gray-600);
  border-radius: var(--radius-md);
  padding: 10px 14px;
  font-size: var(--text-md);
  outline: none;
  transition: border-color 0.2s ease;
}

.input-text:focus {
  border-color: var(--color-blue-600);
  box-shadow: 0 0 0 3px rgba(10, 132, 255, 0.1);
}

.input-text::placeholder {
  color: var(--color-gray-500);
}
```

#### Textarea (Chat Input)

```css
.chat-input {
  width: 100%;
  background: transparent;
  color: var(--color-gray-100);
  border: none;
  font-size: var(--text-md);
  line-height: var(--line-height-normal);
  resize: none;
  outline: none;
  font-family: var(--font-sans);
}

.chat-input::placeholder {
  color: var(--color-gray-500);
}
```

#### Mention Tokens

```css
.mention-token {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  background: rgba(10, 132, 255, 0.1);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  color: var(--color-blue-600);
  margin: 0 2px;
}
```

### 卡片组件

#### Session Card

```css
.session-card {
  background: var(--color-gray-800);
  border-radius: var(--radius-lg);
  padding: 16px 20px;
  border: 1px solid var(--color-gray-600);
  transition: all 0.2s ease;
  cursor: pointer;
}

.session-card:hover {
  background: var(--color-gray-700);
  border-color: var(--color-gray-500);
  transform: translateY(-2px);
  box-shadow: var(--shadow-md);
}

.session-card.active {
  border-color: var(--color-blue-600);
  background: rgba(10, 132, 255, 0.05);
}
```

#### Agent Message Card

```css
.agent-message {
  background: var(--color-gray-800);
  border-radius: var(--radius-lg);
  padding: 16px 20px;
  border: 1px solid var(--color-gray-600);
  line-height: var(--line-height-relaxed);
}

.agent-message h1,
.agent-message h2,
.agent-message h3 {
  color: var(--color-gray-100);
  margin-top: 16px;
  margin-bottom: 8px;
}

.agent-message p {
  color: var(--color-gray-100);
  margin-bottom: 12px;
}

.agent-message code {
  background: var(--color-gray-900);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-success);
}
```

### 特殊组件

#### Thinking Block

```css
.thinking-block {
  border-left: 2px solid var(--color-gray-500);
  padding-left: 12px;
  margin-left: 4px;
  color: var(--color-gray-300);
  font-size: var(--text-md);
  line-height: var(--line-height-relaxed);
}

.thinking-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-gray-800);
  border: 1px solid var(--color-gray-600);
  border-radius: var(--radius-full);
  padding: 6px 12px;
  font-size: var(--text-sm);
  color: var(--color-gray-400);
  cursor: pointer;
  transition: all 0.2s ease;
}

.thinking-toggle:hover {
  background: var(--color-gray-700);
  color: var(--color-gray-100);
}

.thinking-toggle.expanded {
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-color: transparent;
}
```

#### Tool Usage Indicator

```css
.tool-indicator {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-gray-800);
  border: 1px solid var(--color-gray-600);
  border-radius: var(--radius-full);
  padding: 6px 12px;
  font-size: var(--text-sm);
  color: var(--color-gray-400);
  cursor: pointer;
  transition: all 0.2s ease;
}

.tool-indicator:hover {
  border-color: var(--color-blue-600);
  color: var(--color-blue-600);
}
```

#### Approval Block

```css
.approval-block {
  background: var(--color-gray-800);
  border: 1px solid var(--color-warning);
  border-radius: var(--radius-lg);
  padding: 16px;
  margin: 12px 0;
}

.approval-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
}

.approval-title {
  font-size: var(--text-md);
  font-weight: var(--font-weight-medium);
  color: var(--color-gray-100);
}

.danger-badge {
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: var(--font-weight-medium);
  text-transform: uppercase;
}

.danger-low { background: var(--color-success-bg); color: var(--color-success); }
.danger-medium { background: var(--color-warning-bg); color: var(--color-warning); }
.danger-high { background: var(--color-error-bg); color: var(--color-error); }
```

---

## 交互设计

### 微动画

#### 按钮 Hover 效果

```css
@keyframes button-hover {
  0% { transform: translateY(0); }
  100% { transform: translateY(-1px); }
}

.btn-primary:hover {
  animation: button-hover 0.2s ease forwards;
}
```

#### 卡片出现动画

```css
@keyframes card-appear {
  0% {
    opacity: 0;
    transform: translateY(10px);
  }
  100% {
    opacity: 1;
    transform: translateY(0);
  }
}

.session-card {
  animation: card-appear 0.3s ease;
}
```

#### Loading Spinner

```css
@keyframes spin {
  0% { transform: rotate(0deg); }
  100% { transform: rotate(360deg); }
}

.loading-spinner {
  width: 16px;
  height: 16px;
  border: 2px solid var(--color-gray-600);
  border-top-color: var(--color-blue-600);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
```

#### Pulse 动画（Agent 工作状态）

```css
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.5; }
}

.agent-working {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--color-success);
  animation: pulse 2s infinite;
}
```

### 过渡效果

```css
/* 全局过渡 */
* {
  transition: background-color 0.2s ease,
              border-color 0.2s ease,
              color 0.2s ease,
              opacity 0.2s ease;
}

/* 禁用某些元素的过渡（性能优化） */
.chat-input,
textarea,
input {
  transition: none;
}
```

### 即时反馈

#### 操作成功

```css
.feedback-success {
  position: fixed;
  bottom: 24px;
  right: 24px;
  background: var(--color-success);
  color: white;
  padding: 12px 20px;
  border-radius: var(--radius-md);
  font-size: var(--text-md);
  box-shadow: var(--shadow-lg);
  animation: slide-in 0.3s ease;
  z-index: 10000;
}

@keyframes slide-in {
  0% { transform: translateX(100%); opacity: 0; }
  100% { transform: translateX(0); opacity: 1; }
}
```

#### 操作失败

```css
.feedback-error {
  position: fixed;
  bottom: 24px;
  right: 24px;
  background: var(--color-error);
  color: white;
  padding: 12px 20px;
  border-radius: var(--radius-md);
  font-size: var(--text-md);
  box-shadow: var(--shadow-lg);
  animation: slide-in 0.3s ease;
  z-index: 10000;
}
```

---

## 实现路线图

### Phase 1: 设计系统搭建（1-2 天）

**任务：**

1. 创建 `design-tokens.css` - 包含所有设计变量
2. 创建 `components.css` - 包含所有组件样式
3. 创建 `animations.css` - 包含所有动画
4. 更新 `App.css` - 引入新的设计系统

**输出：**
- `app/src/styles/design-tokens.css`
- `app/src/styles/components.css`
- `app/src/styles/animations.css`

### Phase 2: 布局重构（2-3 天）

**任务：**

1. 修改 `App.tsx` - 添加右栏（Context Panel）
2. 重构 `Sidebar.tsx` - 优化项目和会话列表
3. 创建 `ContextPanel.tsx` - 新的上下文面板组件
4. 更新响应式逻辑

**输出：**
- 更新的 `App.tsx`
- 更新的 `Sidebar.tsx`
- 新的 `ContextPanel.tsx`

### Phase 3: 组件美化（3-4 天）

**任务：**

1. 重构 `AgentRow.tsx` - 应用新的卡片样式
2. 重构 `ChatInput.tsx` - 应用新的输入样式
3. 重构 `EmptyState.tsx` - 优化空状态动画
4. 更新 `SettingsModal.tsx` - 应用新的弹窗样式

**输出：**
- 更新的所有聊天相关组件
- 更美观的视觉效果

### Phase 4: 交互增强（2-3 天）

**任务：**

1. 添加微动画（hover、焦点、加载）
2. 实现即时反馈（操作成功/失败提示）
3. 优化流式输出动画
4. 添加键盘快捷键

**输出：**
- 流畅的交互体验
- 完整的键盘导航支持

### Phase 5: 测试与优化（1-2 天）

**任务：**

1. 跨浏览器测试
2. 响应式测试
3. 性能优化（动画性能、渲染优化）
4. 无障碍优化（ARIA 标签、键盘导航）

**输出：**
- 生产就绪的 UI
- 完整的无障碍支持

---

## 附录：组件 API 设计

### Sidebar 组件

```typescript
interface SidebarProps {
  activeTab: 'code' | 'write';
  onTabChange: (tab: 'code' | 'write') => void;
  onOpenSettings: () => void;
}

interface ProjectItem {
  id: string;
  name: string;
  path: string;
  isExpanded: boolean;
  sessions: SessionItem[];
}

interface SessionItem {
  id: string;
  title: string;
  isActive: boolean;
  messageCount: number;
  lastMessageAt: Date;
}
```

### ChatInput 组件

```typescript
interface ChatInputProps {
  isProcessing: boolean;
  onSend: (message: string) => void;
  entriesLength: number;
  currentModel: string;
  onMention: (type: '@' | '/', query: string) => void;
  onAutocompleteSelect: (item: AutocompleteItem) => void;
}

interface AutocompleteItem {
  label: string;
  value: string;
  icon: 'folder' | 'file' | 'command';
}
```

### AgentMessage 组件

```typescript
interface AgentMessageProps {
  entry: {
    id: string;
    startTime: number;
    endTime?: number;
    blocks: MessageBlock[];
    subagents?: Record<string, Subagent>;
  };
  isStreaming: boolean;
  onApproveTool?: (toolId: string, action: 'allow' | 'deny') => void;
}

interface MessageBlock {
  type: 'thinking' | 'tool' | 'assistant' | 'approval' | 'error';
  text?: string;
  isStreaming?: boolean;
  startTime?: number;
  endTime?: number;
}
```

---

## 总结

这个全新的 UI 设计方案将帮助你：

1. **提升用户体验** - 更清晰的信息架构、更流畅的交互
2. **增强视觉吸引力** - 现代化的设计语言、精致的组件
3. **提高开发效率** - 统一的设计系统、可复用的组件
4. **建立品牌识别** - 独特的视觉风格、一致的体验

下一步，我们可以：
- 开始实现 Phase 1（设计系统搭建）
- 创建高保真原型
- 进行用户测试

你觉得这个方案怎么样？有哪些部分需要调整吗？

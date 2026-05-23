# Agent Core

基于 ReAct 模式的 Rust LLM Agent 系统。

## 快速开始

### 1. 安装

```bash
cargo build --release
```

### 2. 配置

在项目根目录创建 `config.toml`：

```toml
default_model = "deepseek"

[memory]
db_path = "~/.agent_core/memory.db"
embedding_model = "BAAI/bge-small-en-v1.5"
max_core_blocks = 5
default_block_max_chars = 2000
consolidation_enabled = true

[models.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key = "sk-xxxxxxxxxxxxxxxx"
model_id = "deepseek-chat"
max_context_tokens = 65536
temperature = 0.7
react_enabled = true
max_iterations = 10

[models.gpt4o]
base_url = "https://api.openai.com/v1"
api_key = "sk-xxxxxxxxxxxxxxxx"
model_id = "gpt-4o"
max_context_tokens = 128000
temperature = 0.7
react_enabled = true
max_iterations = 10
```

也可以用环境变量引用，避免明文写 API Key：

```toml
[models.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_KEY}"
model_id = "deepseek-chat"
```

然后设置环境变量：

```bash
export DEEPSEEK_KEY="sk-xxxxxxxxxxxxxxxx"
```

### 3. 环境变量方式（不用 config.toml）

如果不想用配置文件，直接设环境变量也行：

```bash
export OPENAI_API_KEY="sk-xxxxxxxxxxxxxxxx"
export OPENAI_BASE_URL="https://api.deepseek.com/v1"
export OPENAI_MODEL="deepseek-chat"
```

### 4. 启动 CLI

```bash
cargo run --release --bin agent-cli
```

## CLI 命令

| 命令 | 说明 |
|------|------|
| `/help` | 显示帮助 |
| `/models` | 列出所有可用模型 |
| `/model <name>` | 切换模型 |
| `/temp <float>` | 设置 temperature，如 `/temp 0.3` |
| `/max-tokens <int>` | 设置最大输出 token，如 `/max-tokens 4096` |
| `/memory` | 查看 Core Memory 内容 |
| `/tokens` | 查看当前对话 token 数 |
| `/clear` | 清空对话历史，开始新 session |
| `/quit` | 退出 |

直接输入文字就是和 agent 对话。

## 配置多个模型

在 `config.toml` 里添加多个 `[models.xxx]` 节：

```toml
default_model = "deepseek"

[models.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_KEY}"
model_id = "deepseek-chat"
max_context_tokens = 65536
temperature = 0.7

[models.gpt4o]
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_KEY}"
model_id = "gpt-4o"
max_context_tokens = 128000
temperature = 0.7

[models.groq-llama]
base_url = "https://api.groq.com/openai/v1"
api_key = "${GROQ_KEY}"
model_id = "llama-3.3-70b-versatile"
max_context_tokens = 32768
temperature = 0.7

[models.local]
base_url = "http://localhost:11434/v1"
api_key = "ollama"
model_id = "qwen2.5:7b"
max_context_tokens = 32768
temperature = 0.7
```

运行时用 `/model groq-llama` 切换。

## 配置参数说明

### 模型配置 `[models.xxx]`

| 参数 | 类型 | 说明 |
|------|------|------|
| `base_url` | string | API 地址，必须是 OpenAI compatible 格式 |
| `api_key` | string | API Key，支持 `${ENV_VAR}` 语法 |
| `model_id` | string | 实际模型 ID，如 `deepseek-chat`、`gpt-4o` |
| `max_context_tokens` | int | 该模型的上下文窗口大小 |
| `temperature` | float | 生成温度，0.0-2.0 |
| `react_enabled` | bool | 是否启用 ReAct 模式（默认 true） |
| `max_iterations` | int | ReAct 最大循环次数（默认 10） |
| `system_prompt` | string | 可选，覆盖默认 ReAct prompt |

### 记忆配置 `[memory]`

| 参数 | 类型 | 说明 |
|------|------|------|
| `db_path` | string | SQLite 数据库路径，支持 `~/` |
| `embedding_model` | string | 嵌入模型名，目前支持 `BAAI/bge-small-en-v1.5` |
| `max_core_blocks` | int | Core Memory 最大块数 |
| `default_block_max_chars` | int | 每个 Core Memory 块的字符上限 |
| `consolidation_enabled` | bool | 是否启用后台记忆去重 |

## 作为库使用

```rust
use agent_core::{AgentBuilder, AgentEvent, Tool};
use serde_json::Value;
use async_trait::async_trait;

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "我的自定义工具" }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"]
        })
    }
    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let input = args["input"].as_str().unwrap();
        Ok(format!("处理完成: {input}"))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = AgentBuilder::from_config("config.toml")?
        .with_tool(MyTool)
        .build()?;

    // 简单调用
    let answer = agent.run("你好").await?;
    println!("{answer}");

    // 带事件回调
    agent.run_with_events("帮我执行任务", |event| {
        match event {
            AgentEvent::Thought(t) => print!("{t}"),
            AgentEvent::ToolStart(name) => println!("\n⚡ 调用 {name}"),
            AgentEvent::ToolResult(r) => println!(" → {r}"),
            AgentEvent::FinalAnswer(a) => println!("\n✅ {a}"),
            AgentEvent::Error(e) => eprintln!("\n❌ {e}"),
        }
    }).await?;

    Ok(())
}
```

## 内置工具

### 文件操作

| 工具 | 说明 |
|------|------|
| `read_file` | 读取文件内容 |
| `write_file` | 写入文件 |
| `grep` | 搜索文件内容（支持正则，类似 `grep -rn`） |
| `glob` | 按模式查找文件（类似 `find`，如 `**/*.rs`） |

### 终端

| 工具 | 说明 |
|------|------|
| `run_command` | 执行 shell 命令，返回 stdout/stderr（超时 60 秒） |

### Git

| 工具 | 说明 |
|------|------|
| `git_status` | 查看工作区状态（`git status --short`） |
| `git_diff` | 查看变更（支持 `--staged` 和指定文件） |
| `git_log` | 查看提交历史（`git log --oneline`） |
| `git_commit` | 暂存全部变更并提交（`git add -A && git commit -m`） |
| `git_show` | 查看指定 commit 详情 |

### 记忆系统

| 工具 | 说明 |
|------|------|
| `core_memory_append` | 向 Core Memory 追加内容 |
| `core_memory_replace` | 替换 Core Memory 中的内容 |
| `core_memory_read` | 读取 Core Memory |
| `conversation_search` | 语义搜索历史对话 |
| `conversation_search_date` | 按时间范围搜索对话 |
| `archival_memory_insert` | 存入知识到 Archival Memory |
| `archival_memory_search` | 语义搜索 Archival Memory |
| `archival_memory_delete` | 删除 Archival Memory 记录 |

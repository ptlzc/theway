//! LLM E2E 集成测试 — 真实 LLM 网关 + theway 二进制 + 全工具覆盖。
//!
//! 与 bash 版 (tools/e2e-llm/run.sh, 现为薄壳) 对应的 Rust 原生版:
//! 类型安全断言 (标记集合 / 产物文件 / 日志工具痕迹), cargo test 统一生态,
//! `#[ignore]` 门控 — 默认不跑, 显式 `--ignored` 才执行。
//!
//! 运行方式:
//!   THEWAY_E2E_KEY=<api-key> cargo test -p theway --test e2e_llm -- --ignored
//!
//! 可选环境变量:
//!   THEWAY_E2E_BASE_URL  网关地址 (缺省探测 ~/.pi/agent/models.json 的 litellm)
//!   THEWAY_E2E_MODEL     模型 id (缺省 deepseek-v4-flash-max)
//!   THEWAY_BIN           theway 二进制 (缺省 target/release/theway)
//!   BRAVE_SEARCH_API_KEY web_search 后端 key (无则 web_search 断言跳过)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

// ── case 定义 ──────────────────────────────────────────────────────────

struct Case {
    name: &'static str,
    timeout_secs: u64,
    /// 必须出现的完成标记 (agent 逐工具 echo <tool>_ok >> marks.txt)
    marks: &'static [&'static str],
    /// 无 BRAVE_SEARCH_API_KEY 时允许缺失的标记
    optional_marks: &'static [&'static str],
    /// 必须存在的产物文件 (case 工作目录下)
    files: &'static [&'static str],
    /// 必须在 log.txt 中出现的工具调用痕迹 ("⚙ <ev>")
    log_evidence: &'static [&'static str],
    /// 无 BRAVE_SEARCH_API_KEY 时允许缺失的日志证据
    optional_log: &'static [&'static str],
    prompt_file: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "tools-fs",
        timeout_secs: 300,
        marks: &[
            "write_ok",
            "read_ok",
            "edit_ok",
            "ls_ok",
            "grep_ok",
            "find_ok",
            "outline_ok",
        ],
        optional_marks: &[],
        files: &["sample.txt"],
        log_evidence: &[
            "write(", "read(", "edit(", "ls(", "grep(", "find(", "outline(",
        ],
        optional_log: &[],
        prompt_file: "cases/tools-fs.prompt.txt",
    },
    Case {
        name: "tools-exec",
        timeout_secs: 300,
        marks: &[
            "bash_ok",
            "exec_ok",
            "get_output_ok",
            "write_to_process_ok",
            "kill_shell_ok",
        ],
        optional_marks: &[],
        files: &["exec_out.txt"],
        log_evidence: &[
            "bash(",
            "exec(",
            "get_output(",
            "write_to_process(",
            "kill_shell(",
        ],
        optional_log: &[],
        prompt_file: "cases/tools-exec.prompt.txt",
    },
    Case {
        name: "tools-dag",
        timeout_secs: 600,
        marks: &[
            "dag_plan_ok",
            "dag_status_ok",
            "dag_wait_ok",
            "dag_skip_ok",
            "dag_cancel_ok",
            "dag_retry_ok",
            "dag_inspect_ok",
        ],
        optional_marks: &[],
        files: &["dag_terminal.txt"],
        log_evidence: &[
            "dag_plan(",
            "dag_status(",
            "dag_wait(",
            "dag_skip(",
            "dag_cancel(",
            "dag_retry(",
            "dag_inspect(",
        ],
        optional_log: &[],
        prompt_file: "cases/tools-dag.prompt.txt",
    },
    Case {
        name: "tools-subagent",
        timeout_secs: 300,
        marks: &["subagent_ok"],
        optional_marks: &[],
        files: &["subagent_out.txt"],
        log_evidence: &["subagent("],
        optional_log: &[],
        prompt_file: "cases/tools-subagent.prompt.txt",
    },
    Case {
        name: "tools-skills",
        timeout_secs: 300,
        marks: &[
            "skill_ok",
            "setskillstate_ok",
            "removeskill_ok",
            "memory_ok",
        ],
        optional_marks: &[],
        files: &[],
        log_evidence: &["Skill(", "SetSkillState(", "RemoveSkill(", "memory("],
        optional_log: &[],
        prompt_file: "cases/tools-skills.prompt.txt",
    },
    Case {
        name: "tools-automation",
        timeout_secs: 300,
        marks: &[
            "newcronjob_ok",
            "listcronjobs_ok",
            "removecronjob_ok",
            "setcronjobstate_ok",
            "newtrigger_ok",
            "listtriggers_ok",
            "removetrigger_ok",
            "settriggerstate_ok",
        ],
        optional_marks: &[],
        files: &[],
        log_evidence: &[
            "NewCronJob(",
            "ListCronJobs(",
            "RemoveCronJob(",
            "SetCronJobState(",
            "NewTrigger(",
            "ListTriggers(",
            "RemoveTrigger(",
            "SetTriggerState(",
        ],
        optional_log: &[],
        prompt_file: "cases/tools-automation.prompt.txt",
    },
    Case {
        name: "tools-web",
        timeout_secs: 300,
        marks: &["web_fetch_ok"],
        optional_marks: &["web_search_ok"],
        files: &[],
        log_evidence: &["web_fetch("],
        optional_log: &["web_search("],
        prompt_file: "cases/tools-web.prompt.txt",
    },
    Case {
        name: "goal",
        timeout_secs: 300,
        marks: &["goal_ok"],
        optional_marks: &[],
        files: &["goal.txt"],
        log_evidence: &["/goal "],
        optional_log: &[],
        prompt_file: "cases/goal.prompt.txt",
    },
];

// ── 环境 / 配置 ─────────────────────────────────────────────────────────

struct E2eConfig {
    api_key: String,
    base_url: String,
    model: String,
    bin: PathBuf,
    brave_key: Option<String>,
}

fn probe_litellm_config() -> (String, String) {
    let path = home_dir().join(".pi/agent/models.json");
    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return (String::new(), String::new()),
    };
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    let lit = &v["providers"]["litellm"];
    (
        lit["apiKey"].as_str().unwrap_or("").to_string(),
        lit["baseUrl"].as_str().unwrap_or("").to_string(),
    )
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"))
}

fn e2e_config() -> Option<E2eConfig> {
    let api_key = env::var("THEWAY_E2E_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("OPENAI_API_KEY").ok().filter(|s| !s.is_empty()));
    let (probe_key, probe_url) = probe_litellm_config();
    let api_key = api_key.unwrap_or(probe_key);
    if api_key.is_empty() {
        return None;
    }
    let base_url = env::var("THEWAY_E2E_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or(probe_url);
    let model = env::var("THEWAY_E2E_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "deepseek-v4-flash-max".to_string());
    let bin = env::var("THEWAY_BIN")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/theway")
        });
    if !bin.exists() {
        panic!("theway binary not found at {bin:?} — build with: cargo build --release");
    }
    Some(E2eConfig {
        api_key,
        base_url,
        model,
        bin,
        brave_key: env::var("BRAVE_SEARCH_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
    })
}

/// 确保 ~/.theway/models.json 注册了目标模型 (OpenAI 兼容端点)。
fn ensure_model_registered(cfg: &E2eConfig) {
    let dir = home_dir().join(".theway");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("models.json");
    let registered = fs::read_to_string(&file)
        .ok()
        .map(|s| s.contains(&format!("\"{}\"", cfg.model)))
        .unwrap_or(false);
    if !registered {
        let json = serde_json::json!({
            "models": [{
                "id": cfg.model,
                "name": format!("{} (e2e)", cfg.model),
                "api": "openai-completions",
                "provider": "openai",
                "baseUrl": cfg.base_url,
                "reasoning": false,
                "input": ["text"],
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0},
                "contextWindow": 128000,
                "maxTokens": 8192,
            }]
        });
        fs::write(&file, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
}

// ── 单 case 执行 ───────────────────────────────────────────────────────

async fn run_case(cfg: &E2eConfig, case: &Case) {
    let prompt = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/e2e_llm")
            .join(case.prompt_file),
    )
    .expect("prompt file");

    // 隔离工作目录: /tmp/e2e-llm/<name> (保留日志便于排查)
    let dir = PathBuf::from("/tmp").join("e2e-llm").join(case.name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let mut child = Command::new(&cfg.bin)
        .args([
            "--provider",
            "openai",
            "--model",
            &cfg.model,
            "--yes",
            "--always-allow",
        ])
        .env("OPENAI_API_KEY", &cfg.api_key)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn theway");

    // 写入 prompt, 关闭 stdin (EOF 触发 REPL 处理)
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(prompt.as_bytes()).await.unwrap();
        stdin.shutdown().await.unwrap();
    }

    // 读管道 + 等退出; child.wait() 是 &mut self, 超时后仍可 kill
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let fut = async {
        let status = child.wait().await?;
        use tokio::io::AsyncReadExt;
        let mut out = Vec::new();
        let mut err = Vec::new();
        stdout.read_to_end(&mut out).await?;
        stderr.read_to_end(&mut err).await?;
        Ok::<_, std::io::Error>((status, out, err))
    };
    let (_status, out, err) = match timeout(Duration::from_secs(case.timeout_secs), fut).await {
        Ok(res) => res.expect("io"),
        Err(_) => {
            let _ = child.kill().await;
            panic!("TIMEOUT after {}s", case.timeout_secs);
        }
    };

    fs::write(dir.join("log.txt"), out.clone()).unwrap();
    if !err.is_empty() {
        fs::write(dir.join("stderr.txt"), err).unwrap();
    }

    let mut failures: Vec<String> = Vec::new();

    // 1) 标记集合
    let marks_raw = fs::read_to_string(dir.join("marks.txt")).unwrap_or_default();
    let marks: Vec<&str> = marks_raw.lines().map(str::trim).collect();
    for m in case.marks {
        if !marks.contains(m) {
            failures.push(format!("missing-mark:{m}"));
        }
    }
    for m in case.optional_marks {
        let required = *m != "web_search_ok" || cfg.brave_key.is_some();
        if required && !marks.contains(m) {
            failures.push(format!(
                "missing-mark:{m} (brave key present: {})",
                cfg.brave_key.is_some()
            ));
        }
    }

    // 2) 产物文件
    for f in case.files {
        if !dir.join(f).exists() {
            failures.push(format!("missing-file:{f}"));
        }
    }

    // 3) 日志工具痕迹 ("⚙ <ev>")
    let log = String::from_utf8_lossy(&out);
    for ev in case.log_evidence {
        if !log.contains(&format!("⚙ {ev}")) {
            failures.push(format!("no-log-evidence:{ev}"));
        }
    }
    for ev in case.optional_log {
        let required = *ev != "web_search(" || cfg.brave_key.is_some();
        if required && !log.contains(&format!("⚙ {ev}")) {
            failures.push(format!(
                "no-log-evidence:{ev} (brave key present: {})",
                cfg.brave_key.is_some()
            ));
        }
    }

    if !failures.is_empty() {
        panic!("FAIL {} — {}", case.name, failures.join(" "));
    }
    println!("PASS {}", case.name);
}

// ── 测试 ───────────────────────────────────────────────────────────────

macro_rules! e2e_test {
    ($case:ident) => {
        #[tokio::test]
        #[ignore = "requires THEWAY_E2E_KEY and a live LLM gateway"]
        async fn $case() {
            let cfg = match e2e_config() {
                Some(c) => c,
                None => {
                    eprintln!("SKIP (no THEWAY_E2E_KEY / litellm gateway)");
                    return;
                }
            };
            ensure_model_registered(&cfg);
            let case_name = stringify!($case).replace('_', "-");
            let case = CASES.iter().find(|c| c.name == case_name).unwrap();
            run_case(&cfg, case).await;
        }
    };
}

e2e_test!(tools_fs);
e2e_test!(tools_exec);
e2e_test!(tools_dag);
e2e_test!(tools_subagent);
e2e_test!(tools_skills);
e2e_test!(tools_automation);
e2e_test!(tools_web);
e2e_test!(goal);

//! 单元测试（一）：拉起时机 / argv / 坏配置 / 引擎预检。
//!
//! 口径逐条见 docs/plans/PROMPT-cosyvoice3-settings-ia-2026-10-09.md；
//! 共享假件在 tests_support，命令与快照在 tests_command。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::tests_support::*;

// ─────────────────────────────────────────────────────────── 拉起时机

#[test]
fn on_apply_spawns_only_on_apply() {
    for cause in [StartCause::Boot, StartCause::Enable] {
        let log = Arc::new(SpawnerLog::default());
        let (mut rt, applied) = runtime_with(
            serde_json::json!({ "program": "/bin/echo", "launch_mode": "on_apply" }),
            cause,
            &log,
        );
        start(&mut rt).expect("on_apply 在 Boot/Enable 下必须成功（只是不 spawn）");
        assert_eq!(log.calls(), 0, "cause={cause:?} 不该 spawn");
        let snap = rt.state_json().unwrap();
        assert_eq!(snap["child_running"], serde_json::json!(false));
        assert_eq!(snap["launch_mode"], serde_json::json!("on_apply"));
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    let log = Arc::new(SpawnerLog::default());
    let (mut rt, _) = runtime_with(
        serde_json::json!({ "program": "/bin/echo", "launch_mode": "on_apply" }),
        StartCause::Apply,
        &log,
    );
    start(&mut rt).expect("on_apply + Apply 应成功拉起");
    assert_eq!(log.calls(), 1, "Apply 必须 spawn");
}

#[test]
fn with_app_spawns_on_boot_enable_and_apply() {
    for cause in [StartCause::Boot, StartCause::Enable, StartCause::Apply] {
        let log = Arc::new(SpawnerLog::default());
        let (mut rt, _) = runtime_with(
            serde_json::json!({ "program": "/bin/echo", "launch_mode": "with_app" }),
            cause,
            &log,
        );
        start(&mut rt).expect("with_app 三条路径都应成功拉起");
        assert_eq!(log.calls(), 1, "cause={cause:?} 必须 spawn");
    }
}

/// 缺键 = with_app（2026-10-09 起的新缺省），Boot 直接拉起。
#[test]
fn missing_launch_mode_defaults_to_with_app() {
    let log = Arc::new(SpawnerLog::default());
    let (mut rt, _) = runtime_with(
        serde_json::json!({ "program": "/bin/echo" }),
        StartCause::Boot,
        &log,
    );
    start(&mut rt).unwrap();
    assert_eq!(log.calls(), 1, "键缺失必须按 with_app 处理（开机拉起）");
    assert_eq!(rt.state_json().unwrap()["launch_mode"], "with_app");
}

// ─────────────────────────────────────────────────────────── argv

#[test]
fn argv_is_per_argument_and_never_goes_through_a_shell() {
    let log = Arc::new(SpawnerLog::default());
    let (mut rt, _) = runtime_with(
        serde_json::json!({
            "launch_mode": "with_app",
            "program": "/opt/cv3/engine",
            "args_json": "[\"--model\", \"CosyVoice 2\", \"--tag\", \"a b\", \"--port\", \"8080\"]"
        }),
        StartCause::Boot,
        &log,
    );
    start(&mut rt).unwrap();
    let argv = log.last_request().argv;
    assert_eq!(
        argv,
        vec![
            "/opt/cv3/engine",
            "--model",
            "CosyVoice 2",
            "--tag",
            "a b",
            "--port",
            "8080",
        ],
        "带空格的一项必须仍是一个 argv 元素"
    );
    for forbidden in ["sh", "bash", "-c", "-lc"] {
        assert!(
            !argv.iter().any(|a| a == forbidden),
            "argv 里不得出现 {forbidden}：{argv:?}"
        );
    }
    assert_eq!(
        argv[0], "/opt/cv3/engine",
        "argv[0] 就是 program，没有 shell 前缀"
    );
}

#[test]
fn args_json_parser_rejects_non_string_arrays() {
    assert_eq!(parse_args_json("").unwrap(), Vec::<String>::new());
    assert_eq!(parse_args_json("   ").unwrap(), Vec::<String>::new());
    assert_eq!(
        parse_args_json("[\"a b\"]").unwrap(),
        vec!["a b".to_string()]
    );
    assert!(parse_args_json("{\"a\":1}").is_err(), "对象不是数组");
    assert!(parse_args_json("[1,2]").is_err(), "数字不是字符串");
    assert!(parse_args_json("not json").is_err(), "非法 JSON 必须报错");
}

#[test]
fn workdir_only_set_when_non_empty() {
    let log = Arc::new(SpawnerLog::default());
    let (mut rt, _) = runtime_with(
        serde_json::json!({ "launch_mode": "with_app", "program": "/bin/echo", "workdir": "   " }),
        StartCause::Boot,
        &log,
    );
    start(&mut rt).unwrap();
    assert_eq!(log.last_request().workdir, None, "空白 = 不设 current_dir");

    let log2 = Arc::new(SpawnerLog::default());
    let (mut rt2, _) = runtime_with(
        serde_json::json!({ "launch_mode": "with_app", "program": "/bin/echo", "workdir": "/tmp" }),
        StartCause::Boot,
        &log2,
    );
    start(&mut rt2).unwrap();
    assert_eq!(
        log2.last_request().workdir,
        Some(std::path::PathBuf::from("/tmp"))
    );
}

// ─────────────────────────────────────────────────────────── 缺省程序（仓库内）

/// 空 / 缺 program **不再**是「没有可执行文件」：回落到仓库内的引擎脚本，
/// 且两条路径都在本仓库内（**禁止**回落仓库外的 3-start.sh）。
///
/// **0.4.2 起只在非 Windows 目标成立**：Windows 目标没有随包入口，缺省 program
/// 是空串（见 config.rs 的 cfg(windows) 常量——不能再把编译机的
/// CARGO_MANIFEST_DIR 烤进 exe）。空串由 windows_entry_blocker 说清该填 .exe。
#[test]
fn missing_program_resolves_to_the_repo_engine_script() {
    let cfg = LaunchConfig::from_json(&serde_json::json!({})).unwrap();
    assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    assert_eq!(cfg.mode, LaunchMode::WithApp, "缺省拉起方式已是 with_app");

    #[cfg(not(windows))]
    {
        assert_eq!(cfg.program, COSYVOICE_PROGRAM);
        assert!(
            cfg.program.starts_with(env!("CARGO_MANIFEST_DIR")),
            "缺省程序必须落在本 crate 目录内：{}",
            cfg.program
        );
        assert!(
            !cfg.program.contains("CosyVoice 3.0"),
            "禁止回落到仓库外的 3-start.sh：{}",
            cfg.program
        );
        assert_eq!(cfg.argv().unwrap()[0], COSYVOICE_PROGRAM);
    }
    #[cfg(windows)]
    {
        assert_eq!(cfg.program, "", "Windows 目标缺省入口是空串（0.4.2）");
        assert!(cfg.argv().is_err(), "空 program 不能组装 argv");
    }

    // 显式给的 program 一字不改（用户自带程序，不做预检）。
    let cfg2 = LaunchConfig::from_json(&serde_json::json!({ "program": "/opt/x/py" })).unwrap();
    assert_eq!(cfg2.program, "/opt/x/py");
    assert!(cfg2.engine.is_none(), "用户自带程序不做内置引擎预检");
}

/// MeloTTS 的缺省程序 / 地址与 CosyVoice3 **不同**。
///
/// 程序那条**只在非 Windows 目标成立**（0.4.2：Windows 侧四个常量都是空串，
/// 见 config.rs）；地址是两边都成立的跨平台契约。
#[test]
fn melo_engine_defaults_are_distinct() {
    let cfg = LaunchConfig::from_json_with_engine(&serde_json::json!({}), MELO_ENGINE).unwrap();
    assert_eq!(cfg.base_url, MELO_BASE_URL);
    assert_ne!(cfg.base_url, DEFAULT_BASE_URL, "不得占用 8080");

    #[cfg(not(windows))]
    {
        assert_eq!(cfg.program, MELO_PROGRAM);
        assert!(cfg.program.contains("/melo/"), "{}", cfg.program);
    }
    #[cfg(windows)]
    {
        assert_eq!(cfg.program, "", "Windows 目标缺省入口是空串（0.4.2）");
        assert_eq!(MELO_PROGRAM, "", "MELO_PROGRAM 不得再烤编译机路径");
    }
}

// ─────────────────────────────────────────────────────────── 引擎预检（纯函数）

/// 造一个临时引擎目录（unique 区分用例，避免并行测试互踩）。
fn temp_engine_dir(unique: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("l2d-local-tts-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_file(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, body).expect("write temp file");
}

/// 三条判据各自的失败面（脚本 / 权重目录 / 权重文件 / 后缀）。
///
/// **解释器那一条没有在这里跑**：钉住它必须把进程的 PATH 换掉，而 PATH 是
/// 进程级共享状态，会与并行跑的同进程用例互相影响。它的正例在
/// preflight_accepts_a_prepared_engine（venv 里那份 python 命中）。
#[test]
fn preflight_rejects_missing_script_weights_and_interpreter() {
    let dir = temp_engine_dir("preflight-missing");
    let program = dir.join("start.sh");
    let weights = dir.join("weights");

    // ① 脚本不在。
    let err = preflight_engine(
        &program,
        &weights,
        &[],
        Some("pt"),
        "venv/bin/python",
        "提示",
    )
    .unwrap_err();
    assert!(err.contains("启动脚本不存在"), "got {err}");

    // ② 脚本在、权重目录不在。
    write_file(&program, "#!/bin/sh\n");
    let err = preflight_engine(
        &program,
        &weights,
        &[],
        Some("pt"),
        "venv/bin/python",
        "提示",
    )
    .unwrap_err();
    assert!(err.contains("权重目录不存在"), "got {err}");

    // ③ 目录在、但缺点名文件。
    std::fs::create_dir_all(&weights).expect("weights dir");
    let err = preflight_engine(
        &program,
        &weights,
        &["checkpoint.pth", "config.json"],
        None,
        "venv/bin/python",
        "提示",
    )
    .unwrap_err();
    assert!(
        err.contains("权重文件缺失") && err.contains("checkpoint.pth"),
        "got {err}"
    );

    // ④ 权重齐了、但没有 *.pt（CosyVoice 那条后缀判据）。
    write_file(&weights.join("config.json"), "{}");
    write_file(&weights.join("checkpoint.pth"), "x");
    let err = preflight_engine(
        &program,
        &weights,
        &[],
        Some("pt"),
        "venv/bin/python",
        "提示",
    )
    .unwrap_err();
    assert!(err.contains("没有 *.pt"), "got {err}");

    // ⑤ 提示必须把「下一步做什么」带上（可执行，而不是只说失败）。
    assert!(err.contains("提示"), "失败文案要带 hint：{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preflight_accepts_a_prepared_engine() {
    let dir = temp_engine_dir("preflight-ok");
    let program = dir.join("start.sh");
    let weights = dir.join("weights");
    write_file(&program, "#!/bin/sh\n");
    write_file(&weights.join("llm.pt"), "x");
    // 引擎自带 venv（优先于 PATH，避免依赖跑测机器上的 python）。
    write_file(&dir.join("venv/bin/python"), "#!/bin/sh\n");
    preflight_engine(
        &program,
        &weights,
        &[],
        Some("pt"),
        "venv/bin/python",
        "提示",
    )
    .expect("脚本 + 权重 + 解释器齐备时必须通过");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 仓库里那两份引擎脚本必须**真的在**（缺省 program 不能指向不存在的文件）。
///
/// **Windows 目标不适用**（0.4.2）：那里的缺省 program 是空串，没有仓库内脚本，
/// 由 windows_entry_blocker 在 spawn 前给出「请配 .exe」。
#[cfg(not(windows))]
#[test]
fn repo_engine_scripts_exist() {
    assert!(
        std::path::Path::new(COSYVOICE_PROGRAM).is_file(),
        "CosyVoice3 启动脚本必须在仓库内：{COSYVOICE_PROGRAM}"
    );
    assert!(
        std::path::Path::new(MELO_PROGRAM).is_file(),
        "MeloTTS 启动脚本必须在仓库内：{MELO_PROGRAM}"
    );
}

/// 0.4.2 Windows：四个对象常量都**不再**烤编译机路径。
///
/// 缺陷现场：打包把 crate 解到 `/tmp/live2d-ai-pack.<随机>/` 再编译，
/// `CARGO_MANIFEST_DIR` 被 `concat!` 写进 exe；临时目录删掉即 Failed。
/// 这里钉住 Windows 侧的值就是空串（空 program 由入口判据接管）。
#[cfg(windows)]
#[test]
fn windows_constants_do_not_bake_build_paths() {
    for (name, raw) in [
        ("COSYVOICE_PROGRAM", COSYVOICE_PROGRAM),
        ("COSYVOICE_WEIGHTS_DIR", COSYVOICE_WEIGHTS_DIR),
        ("MELO_PROGRAM", MELO_PROGRAM),
        ("MELO_WEIGHTS_DIR", MELO_WEIGHTS_DIR),
    ] {
        assert_eq!(
            raw, "",
            "{name} 在 Windows 目标上必须是空串（不烤编译机路径）"
        );
    }
}

// ─────────────────────────────────────────────────────────── 坏配置

#[test]
fn illegal_launch_mode_fails_at_create() {
    let applied = Arc::new(AtomicUsize::new(0));
    // Box<dyn ModRuntime> 不是 Debug，所以不用 expect_err。
    let err = match FACTORY.create(
        spy_services(&applied),
        serde_json::json!({ "launch_mode": "sometimes", "program": "/bin/true" }),
    ) {
        Ok(_) => panic!("非法 launch_mode 必须在 create 就失败，禁止折成缺省"),
        Err(e) => e,
    };
    assert!(matches!(err, ModError::Init { .. }), "got {err:?}");
    assert!(err.to_string().contains("sometimes"), "错误要带原值：{err}");

    let err2 = match FACTORY.create(
        spy_services(&applied),
        serde_json::json!({ "launch_mode": 7 }),
    ) {
        Ok(_) => panic!("非字符串 launch_mode 也必须失败"),
        Err(e) => e,
    };
    assert!(matches!(err2, ModError::Init { .. }), "got {err2:?}");

    // 空串同样非法（不是「没填」）。
    assert!(
        FACTORY
            .create(
                spy_services(&applied),
                serde_json::json!({ "launch_mode": "" })
            )
            .is_err()
    );

    // MeloTTS 工厂走同一套解析（同一个 crate，不复制一份判据）。
    assert!(
        MELO_FACTORY
            .create(
                spy_services(&applied),
                serde_json::json!({ "launch_mode": "sometimes" })
            )
            .is_err()
    );
}

#[test]
fn illegal_args_json_only_fails_when_it_really_spawns() {
    let bad = serde_json::json!({
        "program": "/bin/echo",
        "launch_mode": "on_apply",
        "args_json": "{不是 JSON}"
    });

    // on_apply + Boot：**不**失败（这份坏参数这次用不上）。
    let log = Arc::new(SpawnerLog::default());
    let (mut rt, _) = runtime_with(bad.clone(), StartCause::Boot, &log);
    start(&mut rt).expect("不 spawn 的 Boot 不该因为 args_json 失败");
    assert_eq!(log.calls(), 0);

    // on_apply + Apply：这次真要 spawn → 失败。
    let log2 = Arc::new(SpawnerLog::default());
    let (mut rt2, _) = runtime_with(bad, StartCause::Apply, &log2);
    let err = start(&mut rt2).expect_err("真要 spawn 时非法 args_json 必须失败");
    assert!(
        err.to_string().contains("args_json"),
        "错误要指明字段：{err}"
    );
    assert_eq!(log2.calls(), 0, "失败时不得调用 spawner");
}

/// 内置引擎的脚本不在时，**spawn 之前**就必须 Err（不 exec、不重试、不改地址）。
#[test]
fn spawn_is_refused_before_exec_when_the_engine_is_not_prepared() {
    let missing = std::env::temp_dir().join("l2d-local-tts-not-downloaded");
    let _ = std::fs::remove_dir_all(&missing);
    let cfg = LaunchConfig {
        mode: LaunchMode::WithApp,
        program: missing
            .join("engine/start.sh")
            .to_string_lossy()
            .to_string(),
        args_json: "[]".to_string(),
        workdir: String::new(),
        base_url: DEFAULT_BASE_URL.to_string(),
        engine: Some(COSYVOICE_ENGINE),
    };
    let log = Arc::new(SpawnerLog::default());
    let applied = Arc::new(AtomicUsize::new(0));
    set_start_cause(StartCause::Boot);
    let mut rt = LocalTtsRuntime::with_spawner(spy_services(&applied), cfg, log.spawner());
    let err = start(&mut rt).expect_err("脚本不在时必须 Err");
    assert!(err.to_string().contains("启动脚本不存在"), "got {err}");
    assert_eq!(log.calls(), 0, "预检失败不得调用 spawner");
    assert_eq!(applied.load(Ordering::SeqCst), 0, "仍然不写 [tts]");
}

/// 0.4.0 Windows 入口判据（**纯函数**，Linux 上直接测；Windows 上由
/// `cfg(windows)` 的两处调用点生效）。
///
/// 口径：Windows 目标**没有**随包的 Windows 入口（melo/start.sh 与
/// engine/start.sh 都是 bash），失败原因必须可读、且入口必须是 .exe 本身——
/// 不许把脚本交给 CreateProcess。CosyVoice 不因这条恢复。
#[test]
fn windows_entry_blocker_refuses_scripts_and_accepts_exe() {
    use crate::config::windows_entry_blocker;
    use std::path::Path;

    for script in [
        "melo/start.sh",
        "engine/start.sh",
        "run.bat",
        "run.cmd",
        "run.ps1",
        "engine/python",
    ] {
        let reason = windows_entry_blocker(Path::new(script))
            .unwrap_or_else(|| panic!("{script} 必须被 Windows 入口判据拒绝"));
        assert!(
            reason.contains("Windows") && reason.contains(".exe"),
            "错误要可读且指向 .exe：{reason}"
        );
    }
    let empty = windows_entry_blocker(Path::new("")).expect("空入口必须拒绝");
    assert!(empty.contains(".exe"), "空入口的提示也要指向 .exe：{empty}");
    assert!(
        windows_entry_blocker(Path::new(r"C:\tools\melo\python.exe")).is_none(),
        "Windows 入口 = .exe 文件本身"
    );
    assert!(
        windows_entry_blocker(Path::new(r"C:\tools\melo\PYTHON.EXE")).is_none(),
        "扩展名大小写不敏感"
    );
}

/// 0.4.2 缺陷 1：Windows 空入口的提示**不得**泄露编译机路径。
///
/// 现象（0.4.1 的包）：启用 `local-tts-melo` 时报的入口是
/// `/tmp/live2d-ai-pack.n3bheqy_/live2d-ai-mod-local-tts/melo/start.sh`——
/// 打包机临时目录被 `concat!(env!("CARGO_MANIFEST_DIR"), …)` 烤进了 exe，
/// 目录一删就每次 Failed。0.4.2 起 Windows 侧四个常量都是空串，用户看到的是
/// 「请把 program 配成 Windows 可执行文件（.exe）」。
///
/// 这条判据是**纯函数**，Linux 上也能跑（Windows 侧的值由 cfg 分开，
/// 不在本测试覆盖范围；这里锁的是**文案**不许带路径）。
#[test]
fn windows_entry_blocker_empty_message_carries_no_build_paths() {
    use crate::config::windows_entry_blocker;
    use std::path::Path;

    let reason = windows_entry_blocker(Path::new("")).expect("空入口必须拒绝");
    // 0.4.0 那句原文还在（用户据此知道下一步做什么）。
    assert!(
        reason.contains("Windows 目标没有配置引擎入口"),
        "原句必须保留：{reason}"
    );
    assert!(reason.contains(".exe"), "提示要指向 .exe：{reason}");

    for needle in ["live2d-ai-pack", "/tmp", env!("CARGO_MANIFEST_DIR")] {
        assert!(
            !reason.contains(needle),
            "空入口文案不得含 {needle:?}（缺陷 1 的现场）：{reason}"
        );
    }
}
